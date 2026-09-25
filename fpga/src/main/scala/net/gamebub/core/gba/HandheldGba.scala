package net.gamebub.core.gba

import chisel3._
import chisel3.util._
import gba.GBA
import gba.cart.emu.EmulatedCartridge
import HandheldGba.CommandState
import lib.mem.{MemoryArbiter, MemoryInterface, MemoryMap, PipelineInterfaceBridge, PipelineMemoryInterface, RegisterMap}
import lib.mem.PipelineMemoryArbiter
import lib.mem.PipelineMemoryBurstCdc
import lib.mem.cache.DirectReadCache
import lib.mem.sdram.BurstSdramController
import lib.mem.sram.AsyncSramController
import lib.util.ButtonFilter
import lib.video.ColorCorrection
import net.gamebub.framework.Core
import net.gamebub.framework.interface._
import xilinx.MMCM


object HandheldGba {
  /// Single-entry cache that accounts for the fact that sequential 16-bit accesses
  /// (from emulated cartridge) turn into repeated 32-bit SDRAM accesses.
  /// This cache returns the last read if it's in the same 32-bit word.
  private[gba] class MiniCache(addressWidth: Int, dataWidth: Int) extends Module {
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
    val regLastValid = RegInit(false.B)
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
      when (regLastValid && regLastAddress === io.in.address) {
        regBusyLocal := true.B
      } .otherwise {
        regBusyLocal := false.B
        io.out.enable := true.B
        regLastValid := true.B
        regLastAddress := io.out.address
      }
    }
  }

  object CommandState extends ChiselEnum {
    val idle, busy, error, done = Value
  }

  val mmcmVcoHz = 50_000_000.toDouble / 3 * 56.375
}



class HandheldGba extends Module with Core {
  val displayDivider = (HandheldGba.mmcmVcoHz / ClocksV0.getClockDisplayHz(1.0 / 60.0)._1).floor.toInt
  val io = IO(new Bundle {
    val clocks = new ClocksV0(
      // ~ 16.7772 MHz
      clockSystemHz = (HandheldGba.mmcmVcoHz / 56).toInt,
      clockDisplayHz = (HandheldGba.mmcmVcoHz / displayDivider).toInt,
      clockSpiHz = (HandheldGba.mmcmVcoHz / 5).toInt,
    )
    val video = new VideoV0(
      videoWidth = 240,
      videoHeight = 160,
      colorDepthR = 5,
      colorDepthG = 5,
      colorDepthB = 5,
      framePeriod = ((240 + 68) * (160 + 68) * 4).toDouble / (16 * 1024 * 1024),
    )
    val videoFilter = new VideoFilterBasicV0(
      colorInDepthR = 5,
      colorInDepthG = 5,
      colorInDepthB = 5,
      latency = 3,
    )
    val audio = new AudioV0()
    val host = new HostV0()
    val pmod = new PmodV0()
    val input = new InputV0()
    val vibrate = new VibrateV0()
    val cartridge = new CartridgePortV0()
    val link = new LinkPortV0()
    val sram = new SramV0()
    val sdram = new SdramV0()
  })

  // Main MMCM
  val mmcm = Module(new MMCM(
      clockInHz = 50_000_000,
      divide = 3,
      multiply = 56.375,
      clockOutConfig = Seq(
          MMCM.ClockOut(56), // System
          MMCM.ClockOut(14),  // SDRAM (4x)
          MMCM.ClockOut(displayDivider),  // Display
          MMCM.ClockOut(5),   // Host SPI
      )
  ))
  mmcm.io.clockIn := io.clocks.clockIn50M
  mmcm.io.powerDown := false.B
  io.clocks.clockOutSystem := mmcm.io.clockOuts(0)
  val clockSdram = mmcm.io.clockOuts(1)
  val clockSdramHz = io.clocks.clockSystemHz * 4
  io.clocks.clockOutDisplay := mmcm.io.clockOuts(2)
  io.clocks.clockOutSpi := mmcm.io.clockOuts(3)
  io.clocks.locked := mmcm.io.locked

  val regCoreSetup = RegInit(false.B)
  val regCoreReset = RegInit(true.B)
  val regCoreFocus = RegInit(false.B)
  val regCoreResetOnce = RegInit(false.B)
  regCoreResetOnce := false.B

  val configRegEmuCart = RegInit(0.U.asTypeOf(new EmulatedCartridge.Config))
  val configRegRomSize = RegInit(0.U(25.W))
  val configRegGBPlayer = RegInit(0.U(1.W))
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

  // SRAM arbiter (shared between host, EWRAM and emucart)
  val sramArbiter = Module(new MemoryArbiter(addressWidth = 19, dataWidth = 16, n = 3))
  val sramHost = sramArbiter.io.initiator(0)
  val sramEwram = sramArbiter.io.initiator(1)
  val sramEmuCart = sramArbiter.io.initiator(2)

  // SDRAM
  val sdramArbiter = Module(new PipelineMemoryArbiter(addressWidth = 25, dataWidth = 32, n = 2))
  val sdramHost = Wire(new MemoryInterface(addressWidth = 25, dataWidth = 32))
  val sdramEmuCart = sdramArbiter.io.initiator(1)

  {
    val bridge = Module(new PipelineInterfaceBridge(addressWidth = 25, dataWidth = 32))
    bridge.io.source <> sdramHost
    bridge.io.dest <> sdramArbiter.io.initiator(0)
  }

  val registerInterface = Wire(new MemoryInterface(addressWidth = 16, dataWidth = 32))
  val biosInterface = Wire(new MemoryInterface(addressWidth = 14, dataWidth = 32)) // 16 KiB
  val colorCorrectInterface = Wire(new MemoryInterface(addressWidth = 16, dataWidth = 16))
  val commandInterface = Wire(new MemoryInterface(addressWidth = 16, dataWidth = 32))
  val memoryMap = MemoryMap(
    addressWidth = 32,
    dataWidth = 32,
    entries = Seq(
      0x0.U(4.W) -> registerInterface,
      0x1.U(4.W) -> biosInterface,
      0x3.U(4.W) -> sdramHost,
      0x4.U(4.W) -> sramHost,
      0x5.U(4.W) -> colorCorrectInterface,
      0xF0.U(8.W) -> commandInterface,
    ))
  io.host.mem.unsafe :<>= memoryMap.unsafe
  memoryMap.writeStrobe := "b1111".U

  suppressEnumCastWarning {
    registerInterface <> RegisterMap(
      addressWidth = 16,
      dataWidth = 32,
      entries = Seq(
        0x0000 -> RegisterMap.Entry.rw(configRegEmuCart),
        // Rom size (minus one), max (2**25 - 1), 32MiB
        0x0004 -> RegisterMap.Entry.rw(configRegRomSize),
        0x0008 -> RegisterMap.Entry.rw(configRegGBPlayer),
        0x0100 -> RegisterMap.Entry.rw(configRegImuGyroZ),
        0x0104 -> RegisterMap.Entry.rw(configRegImuAccelX),
        0x0108 -> RegisterMap.Entry.rw(configRegImuAccelY),
        0x0200 -> makeRtcAccess(0),
        0x0204 -> makeRtcAccess(1),

        0x1000 -> RegisterMap.Entry.rw(statRegStalls),
        0x1004 -> RegisterMap.Entry.rw(statRegCycles),

        0x2000 -> RegisterMap.Entry.w(regCoreResetOnce),
      )
    )
  }

  // Command interface
  val commandHostState = RegInit(CommandState.idle)
  val regCommandHost = Reg(Vec(2, UInt(32.W)))
  commandInterface <> RegisterMap(
    addressWidth = 16,
    dataWidth = 32,
    entries =
      regCommandHost.zipWithIndex.map { case (reg, i) => (0x0000 + (4 * i) -> RegisterMap.Entry.rw(reg)) }
  )
  // Host -> Core commands
  io.host.commandHost.busy := commandHostState === CommandState.busy
  io.host.commandHost.done := commandHostState === CommandState.done
  io.host.commandHost.error := commandHostState === CommandState.error
  when (io.host.commandHost.request) {
    when (commandHostState === CommandState.idle) {
      val command = regCommandHost(0)(15, 0)
      for (reg <- regCommandHost) {
        reg := 0.U
      }
      commandHostState := CommandState.done
      
      when (command === HostV0.CommandGetStatus.U) {
        when (regCoreSetup) {
          regCommandHost(0) := Mux(regCoreReset, HostV0.StatusCoreHalt.U, HostV0.StatusCoreRun.U)
        } .otherwise {
          // No pre-setup initialization to do.
          regCommandHost(0) := HostV0.StatusSetup.U
        }
      } .elsewhen (command === HostV0.CommandSetupComplete.U) {
        // No post-setup initialization to do.
        regCoreSetup := true.B
      } .elsewhen (command === HostV0.CommandCoreRun.U) {
        regCoreReset := false.B
      } .elsewhen (command === HostV0.CommandCoreHalt.U) {
        regCoreReset := true.B
      } .elsewhen (command === HostV0.CommandNotifyFocus.U) {
        regCoreFocus := regCommandHost(1)(0)
      } .elsewhen (command(15, 8) === 0x03.U) {
        // File command
      } .otherwise {
        // Unknown command
        commandHostState := CommandState.error
      }
    }
  } .otherwise {
    commandHostState := CommandState.idle
  }
  // Core -> Host commands
  io.host.commandCore.request := false.B

  io.vibrate.mode := VibrateV0.Mode.Off

  // SDRAM interface and port
  private val cache = Module(new HandheldGba.MiniCache(addressWidth = 25, dataWidth = 32))
  sdramEmuCart <> cache.io.out
  val sdramPort = cache.io.in
  sdramPort.enable := false.B
  sdramPort.address := DontCare
  sdramPort.isWrite := false.B
  sdramPort.writeStrobe := DontCare
  sdramPort.dataWrite := DontCare

  // Gameboy
  val gba = Module(new GBA)
  when (regCoreReset || regCoreResetOnce) {
    gba.reset := true.B
  }
  val doStall = WireDefault(false.B)
  gba.io.enable := false.B
  when (regCoreFocus) {
    when (doStall) {
      statRegStalls := statRegStalls + 1.U
    }.otherwise {
      gba.io.enable := true.B
      statRegCycles := statRegCycles + 1.U
    }
  }

  gba.io.configGBPlayer := configRegGBPlayer.asBool
  when (gba.io.configGBPlayer && gba.io.gbpRumble) {
    io.vibrate.mode := VibrateV0.Mode.On
  }

  // Emulated cartridge
  val emuCart = Module(new EmulatedCartridge)
  when (regCoreReset || regCoreResetOnce) {
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
  sramEmuCart.address := emuCart.io.backup.address
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
      io.vibrate.mode := VibrateV0.Mode.On
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
  gba.io.keypad.a := io.input.buttons.a
  gba.io.keypad.b := io.input.buttons.b
  gba.io.keypad.l := io.input.buttons.l
  gba.io.keypad.r := io.input.buttons.r
  gba.io.keypad.up := io.input.buttons.up
  gba.io.keypad.down := io.input.buttons.down
  gba.io.keypad.left := io.input.buttons.left
  gba.io.keypad.right := io.input.buttons.right
  gba.io.keypad.start := io.input.buttons.start
  gba.io.keypad.select := io.input.buttons.select

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
  sramEwram.address := Cat(1.U(1.W), gba.io.ewram.address, 0.U(1.W))

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

  // SRAM controller
  val sramController = Module(new AsyncSramController(addressWidth = 18, dataWidth = 16))
  io.sram.ceN := false.B
  io.sram.weN := sramController.io.signals.weN
  io.sram.oeN := sramController.io.signals.oeN
  io.sram.writeMaskN := sramController.io.signals.writeMaskN
  io.sram.address := sramController.io.signals.address
  sramController.io.signals.dataIn := io.sram.dataIn
  io.sram.dataOut := sramController.io.signals.dataOut
  io.sram.dataDir := sramController.io.signals.dataDir
  sramController.io.mem <> sramArbiter.io.target
  sramController.io.mem.address := sramArbiter.io.target.address >> 1

  // SDRAM controller
  withClock(clockSdram) {
    val config = BurstSdramController.Config(
      clockFrequency = clockSdramHz,
      accessLength = 2,
      timeRsc = (2 * 1_000_000_000) / clockSdramHz, /* 2 clocks */
      timeWr = (2 * 1_000_000_000) / clockSdramHz, /* 2 clocks */
      enableBurst = true,
    )
    val controller = Module(new BurstSdramController(config))
    val cdc = Module(new PipelineMemoryBurstCdc(
      addressWidth = 25,
      dataWidth = 32,
      addressBurstIncrement = 4,
      enablePrefetch = true,
    ))
    cdc.io.slowClock := clock
    cdc.io.initiator <> sdramArbiter.io.target
    cdc.io.target <> controller.io.mem
    
    io.sdram.clock := clockSdram
    io.sdram.cke := controller.io.signals.cke

    io.sdram.cs := controller.io.signals.cs
    io.sdram.ras := controller.io.signals.ras
    io.sdram.cas := controller.io.signals.cas
    io.sdram.we := controller.io.signals.we

    io.sdram.dqm := controller.io.signals.dqm
    io.sdram.bank := controller.io.signals.bank
    io.sdram.address := controller.io.signals.address
    controller.io.signals.dataIn := io.sdram.dataIn
    io.sdram.dataOut := controller.io.signals.dataOut
    io.sdram.dataDir := controller.io.signals.dataDir
  }
  
  // Video filter (color correction)
  ColorCorrection.setup(
    clock = clock,
    reset = reset,
    videoFilter = io.videoFilter,
    memInterface = colorCorrectInterface,
  )
}