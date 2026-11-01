package net.gamebub.core.gba

import chisel3._
import _root_.circt.stage.ChiselStage
import chisel3.util.{Cat, SRAM}
import gba._
import gba.apu.ApuOutput
import gba.cart.emu.{EmulatedCartridge, VerificationEmulatedCartridge}
import gba.ppu.PpuOutput
import lib.log.Log
import lib.mem.{MemoryInterface, PipelineMemoryInterface}

object SimGba extends App {
  Log.setDefaultLevel(Log.Level.Warning)
  sys.env.get("LOG_LEVELS") match {
    case Some(value) => Log.setLevelsFromString(value)
    case None =>
  }

  // SIM_PHYSICAL_CART=1: serve ROM like a physical cartridge (see SimGba).
  val physicalCart = sys.env.get("SIM_PHYSICAL_CART").contains("1")
  // SIM_SLOW_CART=1 (with SIM_PHYSICAL_CART): model a slower cartridge ROM.
  val slowCart = sys.env.get("SIM_SLOW_CART").contains("1")

  ChiselStage.emitSystemVerilogFile(
    new SimGba(physicalCart, slowCart),
    args,
    firtoolOpts = Array(
      "--preserve-aggregate=1d-vec",
      "-enable-layers=Verification",
      "-enable-layers=Verification.Assert",
      "-enable-layers=Verification.Assume",
      "-enable-layers=Verification.Cover",
    )
  )
}

/**
 * @param physicalCart Run the cartridge controller in physical-cartridge mode (isEmulated = false),
 *   serving ROM data from the bus signals like a real Game Pak: A0-15 latched when nCS falls and
 *   incremented on each nRD rising edge, A16-23 live. Backup memory (including EEPROM, which is
 *   in the top half of the address space) and GPIO still come from the emulated cartridge.
 * @param slowCart (physical mode) ROM data is only valid once nRD has been low for 2 cycles, or
 *   1 cycle if the data bus was already an input (no bus turnaround); otherwise it's garbage.
 *
 * Based on the simulation model in github.com/ghogan42/gamebub.
 */
class SimGba(physicalCart: Boolean = false, slowCart: Boolean = false) extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val ppu = Output(new PpuOutput)
    val keypad = Input(new Keypad.State)
    val apu = Output(new ApuOutput)

    val configGBPlayer = Input(Bool())

    val emuCartConfig = Input(new EmulatedCartridge.Config)
    val emuCartRomSize = Input(UInt(25.W))
    val emuCartBackup = Flipped(new MemoryInterface(addressWidth = 17, dataWidth = 8))
    val emuCartStall = Output(Bool())
  })

  val gba = Module(new GBA())
  gba.io.enable <> io.enable
  gba.io.configGBPlayer := io.configGBPlayer
  gba.io.ppu <> io.ppu
  gba.io.keypad <> io.keypad
  gba.io.apu <> io.apu

  // BIOS, to be filled in by verilator simulator
  val biosRom = {
    val rom = SyncReadMem(16 * 1024 / 4, UInt(32.W))
    // dontTouch: hack to ensure Chisel doesn't optimize the mem out
    val temp = dontTouch(WireDefault(false.B))
    when (temp) {
      rom.write(0.U, 0.U)
    }
    rom
  }
  gba.io.biosRom.data := biosRom.read(gba.io.biosRom.address, gba.io.biosRom.read)

  // EWRAM
  val ewram = SRAM.masked(128 * 1024, Vec(2, UInt(8.W)), numReadPorts = 0, numWritePorts = 0, numReadwritePorts = 1)
  val ewramPort = ewram.readwritePorts(0)
  ewramPort.address := gba.io.ewram.address
  ewramPort.enable := gba.io.ewram.enable
  ewramPort.isWrite := gba.io.ewram.write
  ewramPort.mask.get := gba.io.ewram.writeStrobe.asBools
  ewramPort.writeData := gba.io.ewram.dataWrite.asTypeOf(ewramPort.writeData)
  gba.io.ewram.dataRead := ewramPort.readData.asUInt
  gba.io.ewram.done := true.B

  // Emulated cartridge
  val emuCart = Module(new EmulatedCartridge)
  emuCart.io.config := io.emuCartConfig
  emuCart.io.romSize := io.emuCartRomSize
  emuCart.io.imuAccelX := 0x3A0.U
  emuCart.io.imuAccelY := 0x3A0.U
  emuCart.io.imuGyroZ := DontCare  // No gyro support in simulator
  emuCart.io.rtcDataWrite := false.B
  emuCart.io.rtcDataIn := DontCare
  emuCart.io.rtcDataSelect := DontCare
  io.emuCartStall := emuCart.io.stall
  gba.io.cartridge <> emuCart.io.interface
  io.emuCartBackup <> emuCart.io.backup
  emuCart.io.interfaceEnable := gba.io.enable

  // Cartridge ROM, to be filled in by verilator simulation
  val cartRom = {
    val rom = SyncReadMem(32 * 1024 * 1024 / 2, UInt(16.W))
    // dontTouch: hack to ensure Chisel doesn't optimize the mem out
    val temp = dontTouch(WireDefault(false.B))
    when (temp) {
      rom.write(0.U, 0.U)
    }
    rom
  }
  // Simulate ROM access
  val romBusy = RegInit(false.B)
  val romBusyAddress = Reg(UInt(24.W))
  val romCounter = Reg(UInt(4.W))
  val romAccessWaitCycles = 1 // minimum 1
  emuCart.io.rom.ready := true.B
  when (romBusy) {
    emuCart.io.rom.ready := false.B
    romCounter := romCounter - 1.U
    when (romCounter === 0.U) {
      romBusy := false.B
      emuCart.io.rom.ready := true.B
    }
  }
  when (emuCart.io.rom.enable && emuCart.io.rom.ready) {
    romCounter := romAccessWaitCycles.U
    romBusy := true.B
    romBusyAddress := emuCart.io.rom.address
  }
  emuCart.io.rom.dataRead := cartRom.read(romBusyAddress, romBusy && romCounter === 1.U)


  if (physicalCart) {
    val cart = gba.io.cartridge
    val prevNCS = RegNext(cart.nCS, true.B)
    val prevNRD = RegNext(cart.nRD, true.B)
    val regLatchedLow = RegInit(0.U(16.W))
    val nextLatchedLow = WireDefault(regLatchedLow)
    val regHigh = RegInit(0.U(8.W))
    val nextHigh = WireDefault(regHigh)
    when (!cart.nCS && prevNCS) {
      nextLatchedLow := cart.ADLoOut
      nextHigh := cart.AHiOut
    } .elsewhen (!cart.nCS && cart.nRD && !prevNRD) {
      nextLatchedLow := regLatchedLow + 1.U
    }
    regLatchedLow := nextLatchedLow
    regHigh := nextHigh
    val regLatchedAddress = Cat(regHigh, regLatchedLow)

    // A16-23 are live address lines: they must not change while nCS is low.
    when (io.enable && !cart.nCS && !prevNCS && cart.AHiDir && cart.AHiOut =/= regHigh) {
      printf(cf"CARTBUS: A16-23 changed during ROM access (latched=${regHigh}%x now=${cart.AHiOut}%x)\n")
    }

    // Read ahead so the data for the latched address is valid the cycle nRD goes low (async ROM).
    val romData = cartRom.read(Cat(nextHigh, nextLatchedLow), true.B)
    val gpioAddress = regLatchedAddress >= (0xC2 / 2).U && regLatchedAddress <= (0xC8 / 2).U
    // EEPROM (for ROMs up to 16 MiB) is selected by the top address bit.
    val eepromAddress = regHigh(7)
    cart.isEmulated := false.B
    // Beyond the end of the ROM, the bus floats to the latched (halfword) address.
    val outOfBounds = Cat(regLatchedAddress, 0.U(1.W)) > io.emuCartRomSize
    val romCartData = Mux(outOfBounds, regLatchedLow, romData)
    val romBusData = WireDefault(romCartData)
    if (slowCart) {
      val regReadLowCycles = RegInit(0.U(4.W))
      regReadLowCycles := Mux(cart.nRD, 0.U, Mux(regReadLowCycles.andR, regReadLowCycles, regReadLowCycles + 1.U))
      val prevDataDirOut = RegNext(cart.ADLoDir, true.B)
      val readLowCycles = regReadLowCycles + 1.U  // including this cycle
      val valid = readLowCycles >= 2.U || !prevDataDirOut
      when (!valid) {
        romBusData := ~romCartData
      }
    }
    when (!cart.nCS && !cart.nRD && !gpioAddress && !eepromAddress) {
      cart.ADLoIn := romBusData
    }

    // Check ROM data completed by the cartridge controller against the ROM image.
    {
      import chisel3.util.experimental.BoringUtils
      val done = BoringUtils.bore(gba.cart.isRequestDone)
      val data = BoringUtils.bore(gba.cart.regReadData)
      val address = BoringUtils.bore(gba.cart.currentAddress)
      val isWrite = BoringUtils.bore(gba.cart.currentIsWrite)
      val check = io.enable && done && !isWrite && Cat(address, 0.U(1.W)) <= io.emuCartRomSize &&
        address(23, 7) =/= 0.U && !address(23)
      val expected = cartRom.read(address, check)
      val regCheck = RegNext(check, false.B)
      val regData = RegNext(data)
      val regAddress = RegNext(address)
      when (regCheck && expected =/= regData) {
        printf(cf"ROMCHECK mismatch addr=0x${regAddress << 1}%x got=0x${regData}%x expected=0x${expected}%x\n")
      }
    }
  }

  // Link
  gba.io.link.in.si := false.B
  gba.io.link.in.so := true.B
  gba.io.link.in.sc := true.B
  gba.io.link.in.sd := true.B
}