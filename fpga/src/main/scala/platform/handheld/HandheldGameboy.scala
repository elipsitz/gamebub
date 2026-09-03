package platform.handheld

import chisel3._
import chisel3.util._
import gameboy.Gameboy
import gameboy.cart.emu.{EmuCartConfig, EmuCartridge, Mbc3RtcAccess, RtcState}
import lib.mem.{MemoryInterface, MemoryMap, PipelineInterfaceBridge, RegisterMap}
import lib.util.ButtonFilter
import lib.video.ColorARGB
import net.gamebub.framework.interface._

object HandheldGameboy {
  class Config extends Bundle {
    val isCgb = Bool()
  }
}

/**
 * Clocked by the 8.3886 MHz "Gameboy" clock.
 */
class HandheldGameboy extends Module with HandheldModule {
  val io = IO(new HandheldIo {
    val clocks = new ClocksFixedV0(sysDivider = 112, sdramDivider = 28)
    val video = new VideoV0(
      videoWidth = 160,
      videoHeight = 144,
      colorDepth = 5,
      framePeriod = (456 * 154).toDouble / (4 * 1024 * 1024),
    )
    val audio = new AudioV0()
    val host = new HostV0()
    val pmod = new PmodV0()
    val input = new InputV0()
    val cartridge = new CartridgePortV0()
    val link = new LinkPortV0()
    val sram = new SramV0()
    val sdram = new SdramV0(sdramBurst = false)
  })

  // Config
  val configRegSystem = RegInit(0.U.asTypeOf(new HandheldGameboy.Config))
  val configRegEmuCart = RegInit(0.U.asTypeOf(new EmuCartConfig))
  val configRegRomAddress = RegInit(0.U(19.W))
  val configRegRomMask = RegInit(0.U(23.W))
  val configRegRamAddress = RegInit(0.U(19.W))
  val configRegRamMask = RegInit(0.U(17.W))
  val configRegImuAccelX = RegInit(0.U(16.W))
  val configRegImuAccelY = RegInit(0.U(16.W))
  val configRegDmgOffColor = RegInit(0.U(16.W))
  val configRegSgbButtons = RegInit(0.U(1.W))
  val statRegStalls = RegInit(0.U(32.W))
  val statRegCycles = RegInit(0.U(32.W))

  val emuCartRtcAccess = Wire(new Mbc3RtcAccess)
  emuCartRtcAccess.writeEnable := false.B
  emuCartRtcAccess.writeState := DontCare
  emuCartRtcAccess.latchSelect := DontCare
  private def makeRtcAccess(latched: Boolean): RegisterMap.Entry = {
    RegisterMap.Entry(
      (new RtcState).getWidth,
      read = RegisterMap.ReadFn((read: Bool) => {
        when (read) { emuCartRtcAccess.latchSelect := latched.B }
        emuCartRtcAccess.readState.asUInt
      }),
      write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
        when (write) {
          emuCartRtcAccess.latchSelect := latched.B
          emuCartRtcAccess.writeState := data.asTypeOf(new RtcState)
          emuCartRtcAccess.writeEnable := true.B
        }
      ),
    )
  }

  val registerInterface = Wire(new MemoryInterface(addressWidth = 16, dataWidth = 32))
  val biosInterface = Wire(new MemoryInterface(addressWidth = 11, dataWidth = 8)) // 2 KiB
  val dmgPaletteInterface = Wire(new MemoryInterface(addressWidth = 5, dataWidth = 16))
  io.host.mem <> MemoryMap(
    addressWidth = 24,
    dataWidth = 32,
    entries = Seq(
      0x0.U(4.W) -> registerInterface,
      0x1.U(4.W) -> biosInterface,
      0x2.U(4.W) -> dmgPaletteInterface,
    ))

  suppressEnumCastWarning {
    registerInterface <> RegisterMap(
      addressWidth = 16,
      dataWidth = 32,
      entries = Seq(
        0x0000 -> RegisterMap.Entry.rw(configRegSystem),
        0x0004 -> RegisterMap.Entry.rw(configRegEmuCart), // Suppressing mbcType enum cast
        0x0008 -> RegisterMap.Entry.rw(configRegRomAddress),
        0x000C -> RegisterMap.Entry.rw(configRegRomMask),
        0x0010 -> RegisterMap.Entry.rw(configRegRamAddress),
        0x0014 -> RegisterMap.Entry.rw(configRegRamMask),
        0x0018 -> makeRtcAccess(latched = false),
        0x001C -> makeRtcAccess(latched = true),
        0x0020 -> RegisterMap.Entry.rw(configRegImuAccelX),
        0x0024 -> RegisterMap.Entry.rw(configRegImuAccelY),
        0x0030 -> RegisterMap.Entry.w(configRegDmgOffColor),
        0x0034 -> RegisterMap.Entry.rw(configRegSgbButtons)

        0x1000 -> RegisterMap.Entry.rw(statRegStalls),
        0x1004 -> RegisterMap.Entry.rw(statRegCycles),
      )
    )
  }

  val dmgPalette = Reg(Vec(16, UInt(15.W)))
  when (dmgPaletteInterface.enable && dmgPaletteInterface.write) {
    dmgPalette(dmgPaletteInterface.address(4, 1)) := dmgPaletteInterface.dataWrite
  }
  dmgPaletteInterface.dataRead := 0.U
  dmgPaletteInterface.done := RegNext(dmgPaletteInterface.enable)

  // Gameboy
  val gameboyConfig = Gameboy.Configuration(
    skipBootrom = false,
    optimizeForSimulation = false,
  )
  val gameboy = Module(new Gameboy(gameboyConfig))
  when (io.host.reset) {
    gameboy.reset := true.B
  }
  gameboy.io.isCgb := configRegSystem.isCgb

  // Gameboy clock control
  val doStall = WireDefault(false.B)
  gameboy.io.clockConfig.enable := false.B
  when (io.host.enable) {
    when (doStall) {
      statRegStalls := statRegStalls + 1.U
    }.otherwise {
      gameboy.io.clockConfig.enable := true.B
      statRegCycles := statRegCycles + 1.U
    }
  }
  gameboy.io.clockConfig.provide8Mhz := true.B

  val buttonFilter = Module(new ButtonFilter(new InputV0.Buttons))
  buttonFilter.io.enable := io.host.enable
  buttonFilter.io.input := io.input.buttons
  val sgbButtons = configRegSgbButtons(0)
  gameboy.io.joypad.a := (Mux(sgbButtons, buttonFilter.io.output.b, buttonFilter.io.output.a)
  gameboy.io.joypad.b := (Mux(sgbButtons, buttonFilter.io.output.y, buttonFilter.io.output.b)
  gameboy.io.joypad.up := buttonFilter.io.output.up
  gameboy.io.joypad.down := buttonFilter.io.output.down
  gameboy.io.joypad.left := buttonFilter.io.output.left
  gameboy.io.joypad.right := buttonFilter.io.output.right
  gameboy.io.joypad.start := buttonFilter.io.output.start
  gameboy.io.joypad.select := buttonFilter.io.output.select

  // Vibration unused by default.
  io.input.vibrate := HandheldVibrate.Off

  // PMOD unused
  io.pmod.out := DontCare
  io.pmod.dir := 0.U(4.W)

  io.audio.left := gameboy.io.apu.left << 6
  io.audio.right := gameboy.io.apu.right << 6

  // Link port
  io.link.soOut := gameboy.io.serial.out
  io.link.soDir := true.B
  gameboy.io.serial.in := RegNext(RegNext(io.link.siIn))
  io.link.siOut := DontCare
  io.link.siDir := false.B
  io.link.sdOut := DontCare
  io.link.sdDir := false.B
  gameboy.io.serial.clockIn := RegNext(RegNext(io.link.scIn))
  io.link.scOut := gameboy.io.serial.clockOut
  io.link.scDir := gameboy.io.serial.clockEnable

  // Video output
  val videoX = RegInit(0.U(8.W))
  val videoY = RegInit(0.U(8.W))
  io.video.dataEnable := false.B
  io.video.data.a := DontCare
  io.video.data.r := DontCare
  io.video.data.g := DontCare
  io.video.data.b := DontCare
  val regDisplayOff = RegInit(false.B)

  when (regDisplayOff) {
    // Render a frame of "lcd off" color
    // When the display is turned off, it remains off until the next vblank when
    // the LCD is on. This ensures that the entire screen is blanked, and matches
    // Game Boy behavior.

    io.video.vblank := false.B
    io.video.hblank := false.B
    when (configRegSystem.isCgb) {
      io.video.data.r := 0x1F.U(5.W)
      io.video.data.g := 0x1F.U(5.W)
      io.video.data.b := 0x1F.U(5.W)
    } .otherwise {
      io.video.data := configRegDmgOffColor.asTypeOf(ColorARGB.rgb555())
    }

    when (videoY === 144.U) {
      io.video.vblank := true.B
    } .elsewhen (videoX === 160.U) {
      io.video.hblank := true.B
      videoX := 0.U
      videoY := videoY + 1.U
    } .otherwise {
      io.video.dataEnable := true.B
      videoX := videoX + 1.U
    }

    // Only end blanking after the *next* vblank when the LCD is on
    when (gameboy.io.ppu.lcdEnable && gameboy.io.ppu.vblank) {
      regDisplayOff := false.B
    }
  } .otherwise {
    io.video.vblank := gameboy.io.ppu.vblank
    io.video.hblank := gameboy.io.ppu.hblank

    when (!gameboy.io.clockConfig.enable) {
      // Do nothing.
    } .elsewhen (!gameboy.io.ppu.lcdEnable) {
      // Blank for at least a whole frame.
      regDisplayOff := true.B
      videoX := 0.U
      videoY := 0.U
    } .elsewhen (gameboy.io.ppu.valid) {
      io.video.dataEnable := true.B

      when (configRegSystem.isCgb) {
        io.video.data.r := gameboy.io.ppu.pixel(4, 0)
        io.video.data.g := gameboy.io.ppu.pixel(9, 5)
        io.video.data.b := gameboy.io.ppu.pixel(14, 10)
      } .otherwise {
        val index = gameboy.io.ppu.dmgColor.asUInt
        io.video.data := dmgPalette(index).asTypeOf(ColorARGB.rgb555())
      }
    }
  }

  // Emulated Cartridge
  val emuCart = Module(new EmuCartridge(8 * 1024 * 1024))
  when (io.host.reset) {
    emuCart.reset := true.B
  }
  emuCart.io.config := configRegEmuCart
  emuCart.io.tCycle := gameboy.io.tCycle
  emuCart.io.rtcAccess <> emuCartRtcAccess
  emuCart.io.imu.x := configRegImuAccelX
  emuCart.io.imu.y := configRegImuAccelY

  val sdramBridge = Module(new PipelineInterfaceBridge(addressWidth = 25, dataWidth = 32))
  sdramBridge.io.dest <> io.sdram.mem
  val sdram = sdramBridge.io.source
  sdram.enable := false.B
  sdram.write := false.B
  sdram.address := DontCare
  sdram.dataWrite := DontCare
  sdram.writeStrobe := DontCare
  io.sram.mem.enable := false.B
  io.sram.mem.write := false.B
  io.sram.mem.address := DontCare
  io.sram.mem.dataWrite := DontCare
  io.sram.mem.writeStrobe := DontCare

  val regEmuCartBusy = RegInit(false.B)
  val regEmuCartDataRead = Reg(UInt(8.W))
  val regEmuCartDataWrite = Reg(UInt(8.W))
  val regEmuCartAddress = Reg(UInt(23.W))
  val regEmuCartIsWrite = Reg(Bool())
  val regEmuCartSelectRom = Reg(Bool())
  val emuCartDataWrite = WireDefault(regEmuCartDataWrite)
  val emuCartIsWrite = WireDefault(regEmuCartIsWrite)
  val emuCartAddress = WireDefault(regEmuCartAddress)
  val emuCartSelectRom = WireDefault(regEmuCartSelectRom)
  val emuCartAccessStart = emuCart.io.dataAccess.enable && !emuCart.reset.asBool
  when (emuCartAccessStart) {
    regEmuCartBusy := true.B

    regEmuCartDataWrite := emuCart.io.dataAccess.dataWrite
    regEmuCartAddress := emuCart.io.dataAccess.address
    regEmuCartIsWrite := emuCart.io.dataAccess.write
    regEmuCartSelectRom := emuCart.io.dataAccess.selectRom

    emuCartDataWrite := emuCart.io.dataAccess.dataWrite
    emuCartAddress := emuCart.io.dataAccess.address
    emuCartIsWrite := emuCart.io.dataAccess.write
    emuCartSelectRom := emuCart.io.dataAccess.selectRom
  }
  emuCart.io.dataAccess.valid := false.B
  emuCart.io.dataAccess.dataRead := regEmuCartDataRead
  when (emuCartAccessStart || regEmuCartBusy) {
    when (emuCartSelectRom) {
      when (emuCartIsWrite) {
        // Don't handle ROM writes.
        emuCart.io.dataAccess.valid := true.B
      } .otherwise {
        sdram.enable := true.B
        sdram.write := false.B
        sdram.address := configRegRomAddress + (Cat(emuCartAddress(22, 2), "b00".U(2.W)) & configRegRomMask)
        emuCart.io.dataAccess.dataRead := sdram.dataRead
          .asTypeOf(Vec(4, UInt(8.W)))(
            emuCartAddress(1, 0)
          )
        emuCart.io.dataAccess.valid := sdram.done
      }
    } .otherwise {
      io.sram.mem.enable := true.B
      io.sram.mem.write := emuCartIsWrite
      io.sram.mem.address := (configRegRamAddress + (Cat(emuCartAddress(16, 1), "b0".U(1.W)) & configRegRamMask)) >> 1
      io.sram.mem.dataWrite := Fill(2, emuCartDataWrite)
      io.sram.mem.writeStrobe := Mux(emuCartAddress(0), "b10".U(2.W), "b01".U(2.W))
      emuCart.io.dataAccess.valid := io.sram.mem.done
      emuCart.io.dataAccess.dataRead := Mux(
        emuCartAddress(0),
        io.sram.mem.dataRead(15, 8),
        io.sram.mem.dataRead(7, 0)
      )
    }
  }
  when (regEmuCartBusy && emuCart.io.dataAccess.valid) {
    regEmuCartBusy := false.B
    regEmuCartDataRead := emuCart.io.dataAccess.dataRead
  }

  when (emuCart.io.config.enabled) {
    io.cartridge.enabled := false.B

    // Connect emulated cartridge
    emuCart.io.cartridge <> gameboy.io.cartridge
    io.input.vibrate := Mux(emuCart.io.rumble, HandheldVibrate.On, HandheldVibrate.Off)
    doStall := emuCart.io.stall

    // Disconnect physical cartridge
    io.cartridge.bank0Out := DontCare
    io.cartridge.bank1Out := DontCare
    io.cartridge.bank2Out := DontCare
    io.cartridge.bank3Out := DontCare
    io.cartridge.pin30Out := DontCare
    io.cartridge.pin31Out := DontCare
    io.cartridge.bank0Dir := false.B
    io.cartridge.bank1Dir := false.B
    io.cartridge.bank2Dir := false.B
    io.cartridge.bank3Dir := false.B
    io.cartridge.pin30Dir := false.B
    io.cartridge.pin31Dir := false.B
  } .otherwise {
    // Cartridge I/O
    io.cartridge.enabled := true.B

    // Bank 0: Data bus
    gameboy.io.cartridge.dataIn := io.cartridge.bank0In
    io.cartridge.bank0Out := gameboy.io.cartridge.dataOut
    io.cartridge.bank0Dir := gameboy.io.cartridge.dataDir

    // Bank 1: Address High
    io.cartridge.bank1Out := gameboy.io.cartridge.address(15, 8)
    io.cartridge.bank1Dir := true.B

    // Bank 2: Address Low
    io.cartridge.bank2Out := gameboy.io.cartridge.address(7, 0)
    io.cartridge.bank2Dir := true.B

    // Bank 3: Control signals (0: nCS, 1: nRD, 2: nWR, 3: PHI)
    io.cartridge.bank3Dir := true.B
    io.cartridge.bank3Out := Cat(
      gameboy.io.cartridge.phi,
      gameboy.io.cartridge.nWR,
      gameboy.io.cartridge.nRD,
      gameboy.io.cartridge.nCS,
    )

    // Pin 30: nRST
    // TODO: open-drain bidirectional
    io.cartridge.pin30Dir := true.B
    io.cartridge.pin30Out := gameboy.io.cartridge.nResetOut
    gameboy.io.cartridge.nResetIn := io.cartridge.pin30In

    // Pin 31: VIN
    io.cartridge.pin31Dir := false.B
    io.cartridge.pin31Out := DontCare

    // Disconnect emulated cartridge
    emuCart.io.cartridge := DontCare
    emuCart.io.cartridge.reqStart := false.B
  }

  // Boot ROM
  val bios = SRAM(2048, UInt(8.W), numReadPorts = 1, numWritePorts = 1, numReadwritePorts = 0)
  bios.writePorts(0).enable := biosInterface.enable && biosInterface.write
  bios.writePorts(0).address := biosInterface.address
  bios.writePorts(0).data := biosInterface.dataWrite
  biosInterface.dataRead := 0.U
  biosInterface.done := RegNext(bios.writePorts(0).enable || bios.readPorts(0).enable)
  bios.readPorts(0).enable := gameboy.io.bootRom.read
  bios.readPorts(0).address := gameboy.io.bootRom.address
  gameboy.io.bootRom.data := bios.readPorts(0).data
}