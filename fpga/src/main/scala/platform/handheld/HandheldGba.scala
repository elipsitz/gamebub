package platform.handheld

import chisel3._
import chisel3.util._
import gba.GBA
import gba.cart.emu.EmulatedCartridge
import lib.mem.cache.DirectReadCache
import lib.mem.{MemoryArbiter, MemoryInterface, MemoryMap, PipelineInterfaceBridge, PipelineMemoryInterface, RegisterMap}
import lib.util.ButtonFilter
import lib.video.ColorARGB
import net.gamebub.framework.interface._


object HandheldGba {
  /// Single-entry cache that accounts for the fact that sequential 16-bit accesses
  /// (from emulated cartridge) turn into repeated 32-bit SDRAM accesses.
  /// This cache returns the last read if it's in the same 32-bit word.
  private class MiniCache(addressWidth: Int, dataWidth: Int) extends Module {
    val io = IO(new Bundle {
      val in = new PipelineMemoryInterface(addressWidth, dataWidth)
      val out = Flipped(new PipelineMemoryInterface(addressWidth, dataWidth))
    })

    io.out.enable := false.B
    io.out.isWrite := false.B
    io.out.address := io.in.address
    io.out.writeStrobe := DontCare
    io.out.dataWrite := DontCare

    val regBusy = RegInit(false.B)
    val regBusyLocal = Reg(Bool())
    val regLastAddress = Reg(UInt(addressWidth.W))
    val regLastData = Reg(UInt(dataWidth.W))

    when (regBusy) {
      when (regBusyLocal) {
        io.in.dataRead := regLastData
        io.in.ready := true.B
        regBusy := false.B
      } .otherwise {
        io.in.ready := io.out.ready
        io.in.dataRead := io.out.dataRead

        when (io.out.ready) {
          regLastData := io.out.dataRead
          regBusy := false.B
        }
      }
    } .otherwise {
      io.in.dataRead := DontCare
      io.in.ready := true.B
    }

    when (io.in.ready && io.in.enable) {
      regBusy := true.B
      when (regLastAddress === io.in.address) {
        regBusyLocal := true.B
      } .otherwise {
        regBusyLocal := false.B
        io.out.enable := true.B
        regLastAddress := io.out.address
      }
    }
  }
}


/**
 * Clocked by a 16777216 Hz clock.
 */
class HandheldGba extends Module with HandheldModule {
  val io = IO(new HandheldIo {
    val clocks = new ClocksFixedV0(sysDivider = 56, sdramDivider = 14)
    val video = new VideoV0(
      videoWidth = 240,
      videoHeight = 160,
      colorDepth = 5,
      framePeriod = ((240 + 68) * (160 + 68) * 4).toDouble / (16 * 1024 * 1024),
    )
    val audio = new AudioV0()
    val host = new HostV0()
    val pmod = new PmodV0()
    val input = new InputV0()
    val cartridge = new CartridgePortV0()
    val link = new LinkPortV0()
    val sram = new SramV0()
    val sdram = new SdramV0(sdramBurst = true)
  })

  val configRegEmuCart = RegInit(0.U.asTypeOf(new EmulatedCartridge.Config))
  val configRegRomSize = RegInit(0.U(25.W))
  val configRegGBPlayer = RegInit(0.U(1.W))
  val configRegSgbButtons = RegInit(0.U(1.W))
  val configRegImuGyroZ = RegInit(0.U(12.W))
  val configRegImuAccelX = RegInit(0.U(12.W))
  val configRegImuAccelY = RegInit(0.U(12.W))
  val statRegStalls = RegInit(0.U(32.W))
  val statRegCycles = RegInit(0.U(32.W))

  val rtcDataSelect = Wire(UInt(1.W))
  val rtcDataWrite = WireDefault(false.B)
  val rtcDataIn = Wire(UInt(32.W))
  val rtcDataOut = Wire(UInt(32.W))
  rtcDataSelect := DontCare
  rtcDataIn := DontCare
  private def makeRtcAccess(select: Int): RegisterMap.Entry = {
    RegisterMap.Entry(
      32,
      read = RegisterMap.ReadFn((read: Bool) => {
        when (read) { rtcDataSelect := select.U }
        rtcDataOut.asUInt
      }),
      write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
        when (write) {
          rtcDataSelect := select.U
          rtcDataIn := data
          rtcDataWrite := true.B
        }
      ),
    )
  }

  val registerInterface = Wire(new MemoryInterface(addressWidth = 16, dataWidth = 32))
  val biosInterface = Wire(new MemoryInterface(addressWidth = 14, dataWidth = 32)) // 16 KiB
  io.host.mem <> MemoryMap(
    addressWidth = 24,
    dataWidth = 32,
    entries = Seq(
      0x0.U(4.W) -> registerInterface,
      0x1.U(4.W) -> biosInterface,
    ))

  suppressEnumCastWarning {
    registerInterface <> RegisterMap(
      addressWidth = 16,
      dataWidth = 32,
      entries = Seq(
        0x0000 -> RegisterMap.Entry.rw(configRegEmuCart),
        // Rom size (minus one), max (2**25 - 1), 32MiB
        0x0004 -> RegisterMap.Entry.rw(configRegRomSize),
        0x0008 -> RegisterMap.Entry.rw(configRegGBPlayer),
        0x000C -> RegisterMap.Entry.rw(configRegSgbButtons),
        0x0100 -> RegisterMap.Entry.rw(configRegImuGyroZ),
        0x0104 -> RegisterMap.Entry.rw(configRegImuAccelX),
        0x0108 -> RegisterMap.Entry.rw(configRegImuAccelY),
        0x0200 -> makeRtcAccess(0),
        0x0204 -> makeRtcAccess(1),

        0x1000 -> RegisterMap.Entry.rw(statRegStalls),
        0x1004 -> RegisterMap.Entry.rw(statRegCycles),
      )
    )
  }

  io.input.vibrate := HandheldVibrate.Off

  // SDRAM interface and port
  private val cache = Module(new HandheldGba.MiniCache(addressWidth = 25, dataWidth = 32))
  io.sdram.mem <> cache.io.out
  val sdramPort = cache.io.in
  sdramPort.enable := false.B
  sdramPort.address := DontCare
  sdramPort.isWrite := false.B
  sdramPort.writeStrobe := DontCare
  sdramPort.dataWrite := DontCare
  
  // SRAM arbiter (shared between EWRAM and emucart)
  val sramArbiter = Module(new MemoryArbiter(addressWidth = 18, dataWidth = 16, n = 2))
  io.sram.mem <> sramArbiter.io.target
  val sramEwram = sramArbiter.io.initiator(0)
  val sramEmuCart = sramArbiter.io.initiator(1)

  // Gameboy
  val gba = Module(new GBA)
  when (io.host.reset) {
    gba.reset := true.B
  }
  val doStall = WireDefault(false.B)
  gba.io.enable := false.B
  when (io.host.enable) {
    when (doStall) {
      statRegStalls := statRegStalls + 1.U
    }.otherwise {
      gba.io.enable := true.B
      statRegCycles := statRegCycles + 1.U
    }
  }

  gba.io.configGBPlayer := configRegGBPlayer.asBool
  when (gba.io.configGBPlayer && gba.io.gbpRumble) {
    io.input.vibrate := HandheldVibrate.On
  }

  // Emulated cartridge
  val emuCart = Module(new EmulatedCartridge)
  when (io.host.reset) {
    emuCart.reset := true.B
  }
  emuCart.io.interfaceEnable := gba.io.enable
  emuCart.io.config := configRegEmuCart
  emuCart.io.romSize := configRegRomSize
  emuCart.io.imuGyroZ := configRegImuGyroZ
  emuCart.io.imuAccelX := configRegImuAccelX
  emuCart.io.imuAccelY := configRegImuAccelY

  emuCart.io.rtcDataWrite := rtcDataWrite
  emuCart.io.rtcDataIn := rtcDataIn
  emuCart.io.rtcDataSelect := rtcDataSelect
  rtcDataOut := emuCart.io.rtcDataOut

  // Convert 16-bit addresses to 32-bit byte addresses
  emuCart.io.rom <> sdramPort
  val emuCartRomAddr = Reg(UInt(1.W)) // Low bit only
  sdramPort.address := emuCart.io.rom.address(23, 1) << 2
  when (emuCart.io.rom.enable) {
    assert(sdramPort.ready)
    emuCartRomAddr := emuCart.io.rom.address(0)
  }
  emuCart.io.rom.dataRead := sdramPort.dataRead.asTypeOf(Vec(2, UInt(16.W)))(emuCartRomAddr(0))

  // Emulated cartridge SRAM: convert 8-bit accesses to 16-bit. Starts at 0 bytes into SRAM (takes 128KiB / 512 KiB).
  val regEmuCartSramByte = RegEnable(emuCart.io.backup.address(0), emuCart.io.backup.enable)
  sramEmuCart.enable := emuCart.io.backup.enable
  sramEmuCart.address := emuCart.io.backup.address >> 1
  sramEmuCart.write := emuCart.io.backup.write
  sramEmuCart.dataWrite := Fill(2, emuCart.io.backup.dataWrite)
  sramEmuCart.writeStrobe := Mux(emuCart.io.backup.address(0), "b10".U(2.W), "b01".U(2.W))
  emuCart.io.backup.done := sramEmuCart.done
  emuCart.io.backup.dataRead := sramEmuCart.dataRead.asTypeOf(Vec(2, UInt(8.W)))(regEmuCartSramByte)

  // Cartridge
  when (configRegEmuCart.enabled) {
    // Connect emulated cartridge
    gba.io.cartridge <> emuCart.io.interface
    doStall := emuCart.io.stall || gba.io.ewramStall

    when (emuCart.io.vibrate) {
      io.input.vibrate := HandheldVibrate.On
    }

    // Disconnect physical cartridge
    io.cartridge.enabled := false.B
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
    doStall := gba.io.ewramStall

    gba.io.cartridge.isEmulated := false.B
    io.cartridge.enabled := true.B
    io.cartridge.bank0Dir := gba.io.cartridge.AHiDir
    io.cartridge.bank0Out := gba.io.cartridge.AHiOut
    gba.io.cartridge.AHiIn := io.cartridge.bank0In
    io.cartridge.bank1Dir := gba.io.cartridge.ADLoDir
    io.cartridge.bank1Out := gba.io.cartridge.ADLoOut(15, 8)
    io.cartridge.bank2Dir := gba.io.cartridge.ADLoDir
    io.cartridge.bank2Out := gba.io.cartridge.ADLoOut(7, 0)
    gba.io.cartridge.ADLoIn := Cat(io.cartridge.bank1In, io.cartridge.bank2In)

    io.cartridge.bank3Dir := true.B
    io.cartridge.bank3Out := Cat(
      gba.io.cartridge.phi,
      gba.io.cartridge.nWR,
      gba.io.cartridge.nRD,
      gba.io.cartridge.nCS,
    )
    io.cartridge.pin30Dir := true.B
    io.cartridge.pin30Out := gba.io.cartridge.nCS2
    io.cartridge.pin31Dir := false.B
    io.cartridge.pin31Out := DontCare
    gba.io.cartridge.IRQ := io.cartridge.pin31In

    // Disconnected emulated cartridge
    emuCart.io.interface.phi := false.B
    emuCart.io.interface.nWR := true.B
    emuCart.io.interface.nRD := true.B
    emuCart.io.interface.nCS := true.B
    emuCart.io.interface.ADLoOut := DontCare
    emuCart.io.interface.ADLoDir := DontCare
    emuCart.io.interface.AHiOut := DontCare
    emuCart.io.interface.AHiDir := DontCare
    emuCart.io.interface.nCS2 := true.B
    emuCart.io.interface.reqStart := false.B
    emuCart.io.interface.reqRom := DontCare
    emuCart.io.interface.reqWrite := DontCare
    emuCart.io.interface.reqAddress := DontCare
    emuCart.io.interface.reqEnd := false.B
  }

  // Video output
  io.video.data.a := DontCare
  io.video.data.r := gba.io.ppu.pixel(4, 0)
  io.video.data.g := gba.io.ppu.pixel(9, 5)
  io.video.data.b := gba.io.ppu.pixel(14, 10)
  io.video.dataEnable := gba.io.enable && gba.io.ppu.valid
  io.video.vblank := gba.io.ppu.vblank
  io.video.hblank := gba.io.ppu.hblank

  // Audio output
  io.audio.left := gba.io.apu.left << 6
  io.audio.right := gba.io.apu.right << 6

  // Keypad
  val buttonFilter = Module(new ButtonFilter(new InputV0.Buttons))
  buttonFilter.io.enable := io.host.enable
  buttonFilter.io.input := io.input.buttons
  val sgbButtons = configRegSgbButtons(0)
  gba.io.keypad.a := (Mux(sgbButtons, buttonFilter.io.output.b, buttonFilter.io.output.a)
  gba.io.keypad.b := (Mux(sgbButtons, buttonFilter.io.output.y, buttonFilter.io.output.b)
  gba.io.keypad.l := buttonFilter.io.output.l
  gba.io.keypad.r := buttonFilter.io.output.r
  gba.io.keypad.up := buttonFilter.io.output.up
  gba.io.keypad.down := buttonFilter.io.output.down
  gba.io.keypad.left := buttonFilter.io.output.left
  gba.io.keypad.right := buttonFilter.io.output.right
  gba.io.keypad.start := buttonFilter.io.output.start
  gba.io.keypad.select := buttonFilter.io.output.select

  // BIOS
  val bios = SRAM(16 * 1024 / 4, UInt(32.W), numReadPorts = 1, numWritePorts = 1, numReadwritePorts = 0)
  bios.writePorts(0).enable := biosInterface.enable && biosInterface.write
  bios.writePorts(0).address := biosInterface.address >> 2
  bios.writePorts(0).data := biosInterface.dataWrite
  biosInterface.dataRead := 0.U
  biosInterface.done := RegNext(bios.writePorts(0).enable || bios.readPorts(0).enable)
  bios.readPorts(0).enable := gba.io.biosRom.read
  bios.readPorts(0).address := gba.io.biosRom.address
  gba.io.biosRom.data := bios.readPorts(0).data

  // EWRAM. Starts at 256KB into the external SRAM.
  sramEwram <> gba.io.ewram
  sramEwram.address := Cat(1.U(1.W), gba.io.ewram.address)

  io.pmod.out := gba.io.link.in.asUInt
  io.pmod.dir := "b1111".U(4.W)

  // Link port
  io.link.scOut := RegNext(gba.io.link.out.sc)
  io.link.sdOut := RegNext(gba.io.link.out.sd)
  io.link.siOut := RegNext(gba.io.link.out.si)
  io.link.soOut := RegNext(gba.io.link.out.so)
  io.link.scDir := RegNext(gba.io.link.dir.sc)
  io.link.sdDir := RegNext(gba.io.link.dir.sd)
  io.link.siDir := RegNext(gba.io.link.dir.si)
  io.link.soDir := RegNext(gba.io.link.dir.so)
  gba.io.link.in.sc := RegNext(RegNext(io.link.scIn))
  gba.io.link.in.sd := RegNext(RegNext(io.link.sdIn))
  gba.io.link.in.si := RegNext(RegNext(io.link.siIn))
  gba.io.link.in.so := RegNext(RegNext(io.link.soIn))
}