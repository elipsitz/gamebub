package gameboy.ppu

import chisel3._
import chisel3.util._
import gameboy.{Clocker, Gameboy, PeripheralAccess}

class PpuOutput extends Bundle {
  /** Output pixel value */
  val pixel = Output(UInt(15.W))
  /** Whether the pixel this clock is valid */
  val valid = Output(Bool())
  /** Whether the PPU is in hblank */
  val hblank = Output(Bool())
  /** Whether the PPU is in vblank */
  val vblank = Output(Bool())
  /** Whether the LCD is enabled */
  val lcdEnable = Output(Bool())
  /** Where the pixel is coming from. Only relevant for DMG. */
  val dmgColor = Output(new Bundle {
    /** 0 for OBJ, 1 for BG */
    val isBg = Bool()
    /** Palette index: OBJ0/1 if obj, BG/Window if bg */
    val paletteIndex = UInt(1.W)
    /** Color in the palette. 0 is dark, 3 is bright. */
    val color = UInt(2.W)
  })
}

object Ppu {
  val Width = 160
  val Height = 144
  val NumScanlines = 154
  val NumTicks = 456
  val OamScanLength = 80
  val FifoSize = 8
  val OamBufferLength = 10

  // Indices of bytes in OAM entries
  val OamByteY = 0
  val OamByteX = 1
  val OamByteTile = 2
  val OamByteAttributes = 3
}

/**
 * BG-style FIFO
 * Can be popped from on the same cycle as it's reloaded.
 */
class PixelFifo[T <: Data](gen: T, mustBeEmpty: Boolean) extends Module {
  val io = IO(new Bundle {
    val reloadData = Input(Vec(8, gen))
    val reloadEnable = Input(Bool())
    val popEnable = Input(Bool())
    val outData = Output(gen)
    val outValid = Output(Bool())
    val reloadAck = Output(Bool())
    val register = Output(Vec(8, gen))
  })

  val register = RegInit(VecInit.fill(8)(0.U.asTypeOf(gen)))
  val length = RegInit(0.U(4.W))
  io.register := register

  // The logicalRegister thing is sort of like a latch?
  if (mustBeEmpty) {
    io.reloadAck := length === 0.U && io.reloadEnable
  } else {
    io.reloadAck := io.reloadEnable
  }
  val logicalRegister = Mux(io.reloadAck, io.reloadData, register)
  val logicalLength = Mux(io.reloadAck, 8.U, length)
  io.outData := logicalRegister(0)
  io.outValid := logicalLength =/= 0.U
  when (logicalLength =/= 0.U && io.popEnable) {
    register := VecInit(logicalRegister.slice(1, 8) ++ Seq(0.U.asTypeOf(gen)))
    length := logicalLength - 1.U
  } .elsewhen (io.reloadAck) {
    register := io.reloadData
    length := 8.U
  }
}

class BgPixel extends Bundle {
  /** Color */
  val color = UInt(2.W)
  /** [CGB only] BG-to-OAM priority (0=Use OAM Priority bit, 1=BG Priority) */
  val bgPriority = Bool()
  /** [CGB only] CGB Palette */
  val paletteCgb = UInt(3.W)
}

class ObjPixel extends Bundle {
  /** Color Index (0 is transparent) */
  val color = UInt(2.W)
  /** Palette number (DMG only) */
  val palette = UInt(1.W)
  /** BG priority (0: obj has priority, 1: bg colors 1-3 have priority) */
  val bgPriority = Bool()
  /** [CGB only] OAM index */
  val oamIndex = UInt(6.W)
  /** [CGB only] CGB Palette */
  val paletteCgb = UInt(3.W)
}

object FetcherState extends ChiselEnum {
  val id0, id1, lo0, lo1, hi0, hi1 = Value
  // In hi1 we try to push to the FIFO until it succeeds
}

class OamBufferEntry extends Bundle {
  /** Whether the entry is valid */
  val valid = Bool()
  /** Index of this object in OAM */
  val index = UInt(6.W)
  /** Scanline minus Object Y (row within the object), [0, 16) */
  val y = UInt(4.W)
  /** Object X */
  val x = UInt(8.W)
}

class TileAttributes extends Bundle {
  /** OBJ: BG and window over OBJ? (0: no, 1: bg colors 1-3 over the OBJ) */
  val priority = Bool()
  /** Vertical flip */
  val flipY = Bool()
  /** Horizontal flip */
  val flipX = Bool()
  /** Palette number (DMG, obj only) */
  val palette = UInt(1.W)
  /** CGB: tile vram bank */
  val vramBank = UInt(1.W)
  /** CGB: palette number */
  val paletteCgb = UInt(3.W)
}

/** In CGB mode, used to index into palette memory. */
class RegisterCgbPaletteIndex extends Bundle {
  /** Increment address writing data if true. */
  val autoIncrement = Bool()
  /** Padding bit. */
  val padding = UInt(1.W)
  /** Palette memory address */
  val address = UInt(6.W)
}

class Ppu(config: Gameboy.Configuration) extends Module {
  val io = IO(new Bundle {
    val clocker = Input(new Clocker)
    val output = new PpuOutput
    val registers = new PeripheralAccess

    /// Whether this is a CGB (regardless of mode)
    val isCgb = Input(Bool())
    /// Whether we're in CGB mode
    val cgbMode = Input(Bool())

    // IRQs
    val vblankIrq = Output(Bool())
    val statIrq = Output(Bool())

    // VRAM access
    /** Whether the PPU is accessing VRAM */
    val vramEnabled = Output(Bool())
    /** Which VRAM address the PPU is accessing */
    val vramAddress = Output(UInt(14.W))
    /** Data read from VRAM (synchronous, in the next clock cycle) */
    val vramDataRead = Input(UInt(8.W))

    // OAM access
    /** Whether the PPU is accessing OAM */
    val oamEnabled = Output(Bool())
    /** Which OAM address the PPU is accessing */
    val oamAddress = Output(UInt(8.W))
    /** Data read from OAM (synchronous, in the next clock cycle) */
    val oamDataRead = Input(UInt(8.W))
  })

  /** Current scanline, [0, 154) */
  val regLy = RegInit(0.U(8.W))
  /** Current tick within scanline, [0, 456) */
  val tick = RegInit(0.U(9.W))
  /** Current number of pixels output in this scanline, [0, 168]. First 8 aren't output (for window and obj x < 8) */
  val regLx = RegInit((Ppu.Width + 8).U(8.W))

  /** $FF40 -- LCDC: LCD Control */
  val regLcdc = RegInit((if (config.skipBootrom) 0x91 else 0x0).U.asTypeOf(new RegisterLcdControl))
  /** $FF41 -- STAT: LCD status */
  val regStat = RegInit(0.U.asTypeOf(new RegisterStatus))
  /** $FF42 -- SCY: Viewport Y position */
  val regScy = RegInit(0.U(8.W))
  /** $FF43 -- SCX: Viewport X position */
  val regScx = RegInit(0.U(8.W))
  /** $FF45 -- LYC: LY compare */
  val regLyc = RegInit(0.U(8.W))
  /** $FF47 -- BGP: BG Palette data */
  val regBgp = RegInit(0.U.asTypeOf(new RegisterPalette))
  /** $FF48 -- OBP0: OBJ Palette 0 data */
  val regObp0 = RegInit(0.U.asTypeOf(new RegisterPalette))
  /** $FF49 -- OBP1: OBJ Palette 1 data */
  val regObp1 = RegInit(0.U.asTypeOf(new RegisterPalette))
  /** $FF4A -- WY: Window Y position */
  val regWy = RegInit(0.U(8.W))
  /** $FF4B -- WX: Window X position (plus 7) */
  val regWx = RegInit(0.U(8.W))

  /** $FF68 -- BCPS (CGB only): BG palette index */
  val regBcps = RegInit(0.U.asTypeOf(new RegisterCgbPaletteIndex))
  /** $FF6A -- OCPS (CGB only): Obj palette index */
  val regOcps = RegInit(0.U.asTypeOf(new RegisterCgbPaletteIndex))
  val cgbPaletteBg = RegInit(VecInit.fill(64)(0.U(8.W)))
  val cgbPaletteObj = RegInit(VecInit.fill(64)(0.U(8.W)))

  // Tick and scanline adjustment
  when (io.clocker.pulse4Mhz) {
    when (tick === (Ppu.NumTicks - 1).U) {
      tick := 0.U
      regLx := 0.U
      when (regLy === (Ppu.NumScanlines - 1).U) {
        regLy := 0.U
      } .otherwise {
        regLy := regLy + 1.U
      }
    } .otherwise {
      tick := tick + 1.U
    }
  }

  // HACK: On the last scanline of vblank (153), LY=153 only for one cycle (at most 1 m-cycle) on DMG (2 on CGB),
  // then it gets reset to 0. There's probably a better way to handle this, but...
  // This affects reading via the LY register and the LY=LYC stat interrupt.
  val lyVisible = Mux(regLy === 153.U && tick > 0.U, 0.U, regLy)

  // 1-hot current state wire
  val stateVblank = regLy >= Ppu.Height.U
  val stateOamSearch = !stateVblank && (tick < Ppu.OamScanLength.U)
  val stateDrawing = !stateVblank && (tick >= Ppu.OamScanLength.U) && (regLx < (Ppu.Width + 8).U)
  val stateHblank = !stateVblank && (regLx === (Ppu.Width + 8).U)
  io.output.hblank := stateHblank
  io.output.vblank := stateVblank
  val lycEqualFlag = regLyc === lyVisible
  io.vramEnabled := stateDrawing && regLcdc.enable
  io.output.lcdEnable := regLcdc.enable

  // Memory-mapped register access
  when (io.registers.enabled && io.registers.write) {
    switch (io.registers.address) {
      is (0x40.U) { regLcdc := io.registers.dataWrite.asTypeOf(new RegisterLcdControl) }
      is (0x41.U) { regStat := io.registers.dataWrite(6, 3).asTypeOf(new RegisterStatus) }
      is (0x42.U) { regScy := io.registers.dataWrite }
      is (0x43.U) { regScx := io.registers.dataWrite }
      is (0x45.U) { regLyc := io.registers.dataWrite }
      // 0x46 -- DMA -- is *not* included
      is (0x47.U) { regBgp := io.registers.dataWrite.asTypeOf(new RegisterPalette) }
      is (0x48.U) { regObp0 := io.registers.dataWrite.asTypeOf(new RegisterPalette) }
      is (0x49.U) { regObp1 := io.registers.dataWrite.asTypeOf(new RegisterPalette) }
      is (0x4A.U) { regWy := io.registers.dataWrite }
      is (0x4B.U) { regWx := io.registers.dataWrite }

      // CGB-only palette registers
      is (0x68.U) {
        when (io.cgbMode) {
          regBcps := io.registers.dataWrite.asTypeOf(new RegisterCgbPaletteIndex)
        }
      }
      is (0x69.U) {
        when (io.cgbMode && !(stateDrawing && regLcdc.enable)) {
          cgbPaletteBg(regBcps.address) := io.registers.dataWrite
          when (regBcps.autoIncrement) { regBcps.address := regBcps.address + 1.U }
        }
      }
      is (0x6A.U) {
        when (io.cgbMode) {
          regOcps := io.registers.dataWrite.asTypeOf(new RegisterCgbPaletteIndex)
        }
      }
      is (0x6B.U) {
        when (io.cgbMode && !(stateDrawing && regLcdc.enable)) {
          cgbPaletteObj(regOcps.address) := io.registers.dataWrite
          when (regOcps.autoIncrement) { regOcps.address := regOcps.address + 1.U }
        }
      }
    }
  }
  io.registers.dataRead := 0xFF.U
  when (io.registers.enabled && !io.registers.write) {
    switch (io.registers.address) {
      is (0x40.U) { io.registers.dataRead := regLcdc.asUInt }
      is (0x41.U) {
        val mode = OHToUInt(Seq(stateHblank, stateVblank, stateOamSearch, stateDrawing))
        io.registers.dataRead := Cat(1.U(1.W), regStat.asUInt, lycEqualFlag, mode)
      }
      is (0x42.U) { io.registers.dataRead := regScy }
      is (0x43.U) { io.registers.dataRead := regScx }
      is (0x44.U) { io.registers.dataRead := lyVisible }
      is (0x45.U) { io.registers.dataRead := regLyc }
      is (0x47.U) { io.registers.dataRead := regBgp.asUInt }
      is (0x48.U) { io.registers.dataRead := regObp0.asUInt }
      is (0x49.U) { io.registers.dataRead := regObp1.asUInt }
      is (0x4A.U) { io.registers.dataRead := regWy }
      is (0x4B.U) { io.registers.dataRead := regWx }

      // CGB-only palette registers
      is (0x68.U) { io.registers.dataRead := regBcps.asUInt }
      is (0x69.U) {
        when (!(stateDrawing && regLcdc.enable)) {
          io.registers.dataRead := cgbPaletteBg(regBcps.address)
        }
      }
      is (0x6A.U) { io.registers.dataRead := regOcps.asUInt }
      is (0x6B.U) {
        when (!(stateDrawing && regLcdc.enable)) {
          io.registers.dataRead := cgbPaletteObj(regOcps.address)
        }
      }
    }
  }
  io.registers.valid :=
    VecInit(
      Seq(0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x47, 0x48, 0x49, 0x4A, 0x4B)
        .map(io.registers.address === _.U)
    ).asUInt.orR ||
      (io.cgbMode && io.registers.address >= 0x68.U && io.registers.address <= 0x6B.U)

  // Interrupt request generation
  io.vblankIrq := io.clocker.pulse4Mhz && !RegEnable(stateVblank, false.B, io.clocker.pulse4Mhz) && stateVblank && regLcdc.enable
  val statInterrupt = Cat(Seq(
    (lycEqualFlag && regStat.interruptLyEnable),
    (stateOamSearch && regStat.interruptOamEnable),
    (stateVblank && regStat.interruptVblankEnable),
    (stateHblank && regStat.interruptHblankEnable),
  )).orR
  io.statIrq := io.clocker.pulse4Mhz && !RegEnable(statInterrupt, false.B, io.clocker.pulse4Mhz) && statInterrupt && regLcdc.enable

  // Background FIFO
  val bgFifo = Module(new PixelFifo(new BgPixel, true))
  bgFifo.io.popEnable := false.B
  bgFifo.io.reloadEnable := false.B
  bgFifo.io.reloadData := DontCare
  val bgScrollCounter = RegInit(0.U(3.W))
  when (io.clocker.pulse4Mhz && tick === 80.U) {
    // Latch the lower 3 bits of SCX for the bg pixel skip register.
    // Ensure we do this right after OAM search in each scanline
    //   (because SCX can be set up to the last dot of OAM search)
    bgScrollCounter := regScx(2, 0)
  }

  // Sprites
  val objFifo = Module(new PixelFifo(new ObjPixel, false))
  objFifo.io.popEnable := false.B
  objFifo.io.reloadEnable := false.B
  objFifo.io.reloadData := DontCare
  val oamBuffer = Reg(Vec(Ppu.OamBufferLength, new OamBufferEntry))
  val oamBufferActive = VecInit((0 until Ppu.OamBufferLength).map(
    i => oamBuffer(i).valid && (regLx >= oamBuffer(i).x) && (regLx < oamBuffer(i).x + 8.U))
  )
  /** Whether there is any sprite in the OAM buffer that is activated */
  val objActive = oamBufferActive.asUInt.orR && regLcdc.objEnable
  /** The index of the sprite (in the OAM buffer) that is activated */
  val objActiveIndex = PriorityEncoder(oamBufferActive)
  /** Whether we're waiting to fetch or actively fetching an object. */
  val objFetchWaiting = Wire(Bool())

  // Window registers
  val windowHitWy = RegInit(false.B)
  val windowActive = RegInit(false.B)
  val windowY = Reg(UInt(8.W))

  // Output pixel logic
  io.output.pixel := DontCare
  io.output.dmgColor := DontCare
  io.output.valid := false.B
  when (io.clocker.pulse4Mhz && regLcdc.enable && stateDrawing && bgFifo.io.outValid) {
    when (bgScrollCounter > 0.U) {
      // If we have bg pixels to discard (for SCX % 8) do that now.
      bgFifo.io.popEnable := true.B
      bgScrollCounter := bgScrollCounter - 1.U
    } .elsewhen (!objFetchWaiting) {
      // CGB: LCDC.bgEnable has a different meaning (sprite priority)
      bgFifo.io.popEnable := true.B
      val bgIndex = Mux(regLcdc.bgEnable || io.cgbMode, bgFifo.io.outData.color, 0.U)
      val bgColorDmg = regBgp.colors(bgIndex)
      objFifo.io.popEnable := true.B
      val objIndex = Mux(objFifo.io.outValid && regLcdc.objEnable, objFifo.io.outData.color, 0.U)
      val objColorDmg = Mux(objFifo.io.outData.palette.asBool, regObp1, regObp0).colors(objIndex)
      // Mix pixels and output
      val bgPriority = Wire(Bool())
      when (io.cgbMode) {
        bgPriority :=
            bgIndex =/= 0.U &&
            regLcdc.bgEnable &&
            (objFifo.io.outData.bgPriority || bgFifo.io.outData.bgPriority)
      } .otherwise {
        bgPriority :=
            bgIndex =/= 0.U &&
            objFifo.io.outData.bgPriority
      }
      val isBg = objIndex === 0.U || bgPriority

      io.output.dmgColor.isBg := isBg
      io.output.dmgColor.paletteIndex := Mux(isBg, windowActive, objFifo.io.outData.palette)
      io.output.dmgColor.color := (~Mux(isBg, bgColorDmg, objColorDmg)).asUInt

      when (io.isCgb) {
        // CGB output
        val bgPalette = Wire(UInt(5.W))
        val objPalette = Wire(UInt(5.W))
        when (io.cgbMode) {
          // Use CGB palettes
          bgPalette := Cat(bgFifo.io.outData.paletteCgb, bgIndex)
          objPalette := Cat(objFifo.io.outData.paletteCgb, objIndex)
        } .otherwise {
          // DMG compatibility mode: index into CGB palettes with DMG palettes
          bgPalette := bgColorDmg
          objPalette := Cat(objFifo.io.outData.palette, objColorDmg)
        }
        val bgColorCgb = Cat(cgbPaletteBg(Cat(bgPalette, 1.U(1.W))), cgbPaletteBg(Cat(bgPalette, 0.U(1.W))))
        val objColorCgb = Cat(cgbPaletteObj(Cat(objPalette, 1.U(1.W))), cgbPaletteObj(Cat(objPalette, 0.U(1.W))))
        io.output.pixel := Mux(isBg, bgColorCgb, objColorCgb)
      } .otherwise {
        // DMG output. Expand the grayscale to 15-bits.
        // Note that DMG grayscale is inverted -- 00 is white, 11 is black.
        val grayPixel = Mux(isBg, bgColorDmg, objColorDmg)
        io.output.pixel := Fill(3, Fill(3, (~grayPixel).asUInt)(5, 1))
      }

      // Skip the first 8 pixels to output
      io.output.valid := regLx >= 8.U
      regLx := regLx + 1.U

//      printf(cf"pix ly=${regLy} lx=${regLx} valid=${io.output.valid} bgcol=${bgColor} objColor=${objColor}\n")
    }
  }

  // OAM and VRAM access are one-cycle delayed. This poses an issue with clock enable,
  // because if we set the address and update some state, and then the next cycle we
  // have clock disabled, the data will be gone by the time we are enabled again and
  // need to use it. The fix for this is to store the addresses in a register, so
  // we keep outputting the same address until we're done with it.
  val vramAddress = WireDefault(0.U(14.W))
  val vramAddressValid = WireDefault(false.B)
  val regVramAddress = RegInit(0.U(14.W))
  io.vramAddress := regVramAddress
  when (vramAddressValid) {
    io.vramAddress := vramAddress
    regVramAddress := vramAddress
  }
  val oamAddress = WireDefault(0.U(8.W))
  val oamAddressValid = WireDefault(false.B)
  val regOamAddress = RegInit(0.U(8.W))
  io.oamAddress := regOamAddress
  when (oamAddressValid) {
    io.oamAddress := oamAddress
    regOamAddress := oamAddress
  }

  // Background fetch logic
  val fetcherState = RegInit(FetcherState.id0)
  val fetcherTileAttrs = Reg(new TileAttributes)
  val fetcherTileId = RegInit(0.U(8.W))
  val fetcherTileLo = RegInit(0.U(8.W))
  val fetcherTileHi = RegInit(0.U(8.W))
  val fetcherBgX = RegInit(0.U(8.W))
  val fetcherIsObj = RegInit(false.B)
  /** if fetcherIsObj, the oam buffer index of the object we're fetching */
  val fetcherObjIndex = Reg(UInt(log2Ceil(Ppu.OamBufferLength).W))
  val fetcherTileRow = Wire(UInt(3.W))
  when (fetcherIsObj) { fetcherTileRow := oamBuffer(fetcherObjIndex).y(2, 0) }
    .elsewhen (windowActive) { fetcherTileRow := windowY(2, 0) }
    .otherwise { fetcherTileRow := (regLy + regScy)(2, 0) }
  val fetcherTileIdLsb = WireDefault(fetcherTileId(0))
  when (fetcherIsObj && regLcdc.objSize.asBool) {
    // For an 8x16 object, the lower bit is 0 for the top half, 1 for the bottom (inverted if flipped)
    fetcherTileIdLsb := oamBuffer(fetcherObjIndex).y(3) ^ fetcherTileAttrs.flipY
  }
  // Bypass the tile attrs register if we're fetching the attrs *this clock*.
  val fetcherTileAttrsBypass = WireDefault(false.B)
  val fetcherTileAttrsEffective = Mux(fetcherTileAttrsBypass,
    io.vramDataRead.asTypeOf(new TileAttributes), fetcherTileAttrs)
  val fetcherTileAddress = Cat(
    Mux(io.cgbMode, fetcherTileAttrsEffective.vramBank, 0.U(1.W)),
    !fetcherIsObj && !(regLcdc.bgTileDataArea | fetcherTileId(7)),
    fetcherTileId(7, 1),
    fetcherTileIdLsb,
    Mux(fetcherTileAttrsEffective.flipY, ~fetcherTileRow, fetcherTileRow),
  )
  val fetcherBgWindowTileIdAddress = Wire(UInt(13.W))
  when (windowActive) {
    // Window mode
    val tileY = windowY(7, 3)
    val tileX = fetcherBgX(7, 3)
    fetcherBgWindowTileIdAddress := Cat("b11".U(2.W), regLcdc.windowTileMapArea, tileY, tileX)
  } .otherwise {
    // Background mode
    val tileY = (regLy + regScy)(7, 3)
    val tileX = (fetcherBgX + regScx)(7, 3)
    fetcherBgWindowTileIdAddress := Cat("b11".U(2.W), regLcdc.bgTileMapArea, tileY, tileX)
  }

  objFetchWaiting := objActive || fetcherIsObj
  when (io.clocker.pulse4Mhz && stateDrawing) {
//    printf(cf"fetcherState=${fetcherState}\n")
    switch(fetcherState) {
      // For the fetcher, we want to ensure that we can push on 'hi1'.
      // We also need to ensure that fetcherTileId is set by lo0.
      is (FetcherState.id0) {
        // Set tile ID address (only relevant for BG/Window)
        vramAddress := Cat(0.U(1.W), fetcherBgWindowTileIdAddress)
        vramAddressValid := true.B

        // Start fetching from OAM (only relevant for OBJ)
        oamAddress := Cat(oamBuffer(fetcherObjIndex).index, Ppu.OamByteTile.U(2.W))
        oamAddressValid := true.B

        // Read the tile attributes we started reading when we started sprite fetch
        // DMG: BG doesn't have attributes, set this to 0
        fetcherTileAttrs := io.oamDataRead.asTypeOf(new TileAttributes)
        when (!fetcherIsObj) { fetcherTileAttrs := 0.U.asTypeOf(new TileAttributes) }
      }
      is (FetcherState.id1) {
        fetcherTileId := Mux(fetcherIsObj, io.oamDataRead, io.vramDataRead)

        // CGB: load BG tile attributes
        vramAddress := Cat(1.U(1.W), fetcherBgWindowTileIdAddress)
        vramAddressValid := true.B
      }
      is (FetcherState.lo0) {
        // CGB: store BG tile attributes
        when (!fetcherIsObj && io.cgbMode) {
          fetcherTileAttrs := io.vramDataRead.asTypeOf(new TileAttributes)
          // We need these attrs to compute fetcherTileAddress, so tell it to bypass the register
          // and just use the io.vramDataRead (for now).
          fetcherTileAttrsBypass := true.B
        }

        vramAddress := Cat(fetcherTileAddress, 0.U(1.W))
        vramAddressValid := true.B
      }
      is (FetcherState.lo1) {
        // Store this, but also start the address hi fetch so the data is stored by hi1.
        // The most significant bit is the first, so we reverse the order if it's *not* flipped.
        fetcherTileLo := Mux(fetcherTileAttrs.flipX, io.vramDataRead, Reverse(io.vramDataRead))
        vramAddress := Cat(fetcherTileAddress, 1.U(1.W))
        vramAddressValid := true.B
      }
      is (FetcherState.hi0) {
        fetcherTileHi := Mux(fetcherTileAttrs.flipX, io.vramDataRead, Reverse(io.vramDataRead))
      }
      is (FetcherState.hi1) {
        // Push!
        when (fetcherIsObj) {
          objFifo.io.reloadEnable := true.B
          objFifo.io.reloadData := VecInit((0 until 8).map(i => {
            val pixel = WireDefault(objFifo.io.register(i))
            val newColor = Cat(fetcherTileHi(i), fetcherTileLo(i))
            // Merge with existing contents.
            // Overwrite if the pixel index is 0,
            //  OR in CGB-mode and this obj has a lower OAM index than the existing one.
            val overwrite = io.cgbMode && (fetcherObjIndex < objFifo.io.register(i).oamIndex) && newColor =/= 0.U
            when (overwrite || objFifo.io.register(i).color === 0.U) {
              pixel.color := newColor
              pixel.palette := fetcherTileAttrs.palette
              pixel.bgPriority := fetcherTileAttrs.priority
              pixel.oamIndex := fetcherObjIndex
              pixel.paletteCgb := fetcherTileAttrs.paletteCgb
            }
            pixel
          }))
        } .otherwise {
          bgFifo.io.reloadEnable := true.B
          bgFifo.io.reloadData := VecInit((0 until 8).map(i => {
            val pixel = Wire(new BgPixel)
            pixel.color := Cat(fetcherTileHi(i), fetcherTileLo(i))
            pixel.bgPriority := fetcherTileAttrs.priority
            pixel.paletteCgb := fetcherTileAttrs.paletteCgb
            pixel
          }))
        }
      }
    }
    when (fetcherState < FetcherState.hi1) {
      fetcherState := fetcherState.next
    }
    // Handle sprite fetch.
    // It's unclear exactly what's supposed to happen here, but it seems:
    //  * a sprite has to be active
    //  * the fetcher must be in the pushing state OR the background fifo isn't empty (?)
    when (objActive && fetcherState === FetcherState.hi1 && bgFifo.io.outValid) {
      fetcherState := FetcherState.id0
      fetcherIsObj := true.B
      fetcherObjIndex := objActiveIndex
      // Mark this object as invalid (now that we're starting the fetch)
      oamBuffer(objActiveIndex).valid := false.B
      // The first fetch is repeated twice (only increment if we're not at the start).
      when (bgFifo.io.reloadAck && regLx =/= 0.U) {
        fetcherBgX := fetcherBgX + 8.U
      }
      // Set the address to start fetching the OAM data
      oamAddress := Cat(oamBuffer(objActiveIndex).index, Ppu.OamByteAttributes.U(2.W))
      oamAddressValid := true.B
    } .elsewhen (fetcherState === FetcherState.hi1 && (bgFifo.io.reloadAck || fetcherIsObj)) {
      // Restart as BG fetcher if 1) we're pushing 2) push was successful
      fetcherState := FetcherState.id0
      fetcherIsObj := false.B
      // The first fetch is repeated twice (only increment if we're not at the start).
      when (bgFifo.io.reloadAck && regLx =/= 0.U) {
        fetcherBgX := fetcherBgX + 8.U
      }
    }
    // Sprite fetch abort (note that this may not be how it actually works)
    when (fetcherIsObj && !regLcdc.objEnable) {
      fetcherState := FetcherState.id0
      fetcherIsObj := false.B
    }
  }

  // Window activation logic
  // XXX: windowHitWy should only activate at the beginning of oam search?
  when (io.clocker.pulse4Mhz) {
    when (stateOamSearch && (regLy === regWy)) { windowHitWy := true.B }
    when (stateDrawing && !windowActive && regLcdc.windowEnable && windowHitWy && regWx < 167.U && regLx >= regWx && !objFetchWaiting) {
      windowActive := true.B
      fetcherState := FetcherState.id0
      fetcherBgX := 0.U
      bgFifo.reset := true.B
      bgScrollCounter := 0.U
    }
  }

  // OAM search logic
  // The last part is a bit of a hack -- OAM here is synchronous,
  // so we need to set up the first read on the last dot of the previous scanline (during hblank)
  io.oamEnabled := regLcdc.enable && !stateVblank && (!stateHblank || tick === (Ppu.NumTicks - 1).U)
  val oamSearchIndex = tick(6, 1) // index of OAM we're currently searching
  val oamBufferLen = RegInit(0.U(4.W)) // number of objs in the current scanline buffer
  val oamBufferSave = RegInit(false.B) // whether we're saving the current object
  when (io.clocker.pulse4Mhz && stateOamSearch) {
    oamAddressValid := true.B
    switch (tick(0).asUInt) {
      //  cycle 0: load obj #n's y, set address to obj #n's x
      is (0.U) {
        val objY = io.oamDataRead
        val objHeight = Mux(regLcdc.objSize.asBool, 16.U, 8.U)
        val objActive = ((regLy + 16.U) >= objY) && ((regLy + 16.U) < (objY + objHeight))
//        printf(cf"obj=$oamSearchIndex objY=$objY ly=$regLy act=$objActive\n")
        oamBufferSave := false.B
        when (objActive && oamBufferLen < Ppu.OamBufferLength.U) {
          oamBufferSave := true.B
          oamBuffer(oamBufferLen).valid := true.B
          oamBuffer(oamBufferLen).index := oamSearchIndex
          oamBuffer(oamBufferLen).y := regLy - objY
        }
        oamAddress := Cat(oamSearchIndex, Ppu.OamByteX.U(2.W))
      }
      //  cycle 1: load obj #n's x, set address to obj #(n+1)'s y
      is (1.U) {
        when (oamBufferSave) {
          oamBuffer(oamBufferLen).x := io.oamDataRead
          oamBufferLen := oamBufferLen + 1.U
        }
        oamAddress := Cat(oamSearchIndex + 1.U, Ppu.OamByteY.U(2.W))
      }
    }
  }

  // On hblank, reset scanline registers
  when (io.clocker.pulse4Mhz && stateHblank) {
    fetcherState := FetcherState.id0
    fetcherBgX := 0.U
    bgFifo.reset := true.B
    objFifo.reset := true.B
    windowActive := false.B
    when (windowActive) {
      windowY := windowY + 1.U
    }
    for (i <- 0 until Ppu.OamBufferLength) {
      oamBuffer(i).valid := false.B
    }
    oamBufferLen := 0.U
    oamAddress := 0.U
    oamAddressValid := true.B
  }
  // On vblank, reset frame registers
  when (io.clocker.pulse4Mhz && stateVblank) {
    windowHitWy := false.B
    windowY := 0.U
  }
  // When LCD/PPU is turned off, reset state
  when (io.clocker.pulse4Mhz && !regLcdc.enable) {
    regLy := 0.U
    tick := Ppu.OamScanLength.U
    regLx := (Ppu.Width + 8).U
  }
}
