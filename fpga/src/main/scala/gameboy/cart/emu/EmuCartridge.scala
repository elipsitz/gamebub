package gameboy.cart.emu

import chisel3._
import chisel3.util._
import gameboy.cart.CartridgeInterface

object MbcType extends ChiselEnum {
  val None = Value
  val Mbc1 = Value
  val Mbc2 = Value
  val Mbc3 = Value
  val Mbc5 = Value
  val Mbc7 = Value
}

class EmuCartConfig extends Bundle {
  /** Whether the cartridge has a rumble feature (MBC5 and MBC7 only) */
  val hasRumble = Bool()
  /** Whether there is an RTC (MBC3 only) */
  val hasRtc = Bool()
  /** Whether there is any RAM (external, or for MBC2, internal)  */
  val hasRam = Bool()
  /** The MBC chip type */
  val mbcType = MbcType()
  /** Whether we're using an emulated cartridge */
  val enabled = Bool()
}

/**
 * An emulated cartridge.
 *
 * Provides the regular Gameboy cartridge interface.
 *
 * Gets the actual data through something with the EmuCartridgeDataAccess interface.
 */
class EmuCartridge(clockRate: Int) extends Module {
  val io = IO(new Bundle {
    /** The current cart configuration (including MBC info) */
    val config = Input(new EmuCartConfig())
    /** Underlying data access interface */
    val dataAccess = new EmuCartridgeDataAccess()
    /** Emulated gameboy cartridge interface */
    val cartridge = Flipped(new CartridgeInterface())
    /** Gameboy t-cycle */
    val tCycle = Input(UInt(2.W))
    /** Whether the previous memory request has not completed by the time the GB needs it. */
    val stall = Output(Bool())


    /** Direct access to MBC3 RTC registers */
    val rtcAccess = new Mbc3RtcAccess
    /** Rumble activation signal */
    val rumble = Output(Bool())
    /** IMU state */
    val imu = Input(new Mbc7ImuState)
  })

  val mbcDirectRamAccess = Wire(Bool())

  val mbc = Module(new EmuMbc(clockRate))
  mbc.io.config := io.config
  mbc.io.mbc.memEnable := io.cartridge.reqStart
  mbc.io.mbc.memWrite := io.cartridge.reqWrite
  mbc.io.mbc.memAddress := io.cartridge.reqAddress
  mbc.io.mbc.memDataWrite := io.cartridge.reqDataWrite
  mbc.io.mbc.selectRom := io.cartridge.reqRom
  io.rtcAccess <> mbc.io.rtcAccess
  io.rumble := mbc.io.rumble
  mbc.io.imu := io.imu

  io.dataAccess.enable := false.B
  io.dataAccess.write := io.cartridge.reqWrite
  io.dataAccess.selectRom := io.cartridge.reqRom
  io.cartridge.dataIn := io.dataAccess.dataRead
  io.dataAccess.dataWrite := io.cartridge.reqDataWrite

  io.stall := false.B
  val regBusy = RegInit(false.B)
  when (io.dataAccess.valid) {
    regBusy := false.B
  } .elsewhen (io.cartridge.reqEnd && regBusy) {
    io.stall := true.B
  }
  when (io.cartridge.reqStart) {
    regBusy := true.B
    io.dataAccess.enable := true.B
  }

  when (io.cartridge.reqRom) {
    io.dataAccess.address := Cat(
      Mux(io.cartridge.reqAddress(14), mbc.io.mbc.bankRom2, mbc.io.mbc.bankRom1),
      io.cartridge.reqAddress(13, 0),
    )

    // Cannot write to ROM.
    io.dataAccess.write := false.B
  } .otherwise {
    io.dataAccess.address := Cat(mbc.io.mbc.bankRam, io.cartridge.reqAddress(12, 0))

    // Don't do data access if we're accessing RAM and RAM is being mapped to the MBC
    //   OR if we're accessing RAM but there's no actual RAM
    when (mbc.io.mbc.ramReadMbc || !io.config.hasRam) {
      // Don't make an internal access, and don't stay in the "busy" state.
      io.dataAccess.enable := false.B
      regBusy := false.B
    }

    when (mbc.io.mbc.ramReadMbc) {
      io.cartridge.dataIn := mbc.io.mbc.memDataRead
    } .elsewhen (io.config.mbcType === MbcType.Mbc2) {
      // Special case for MBC2: only bottom 4 bits are valid
      io.cartridge.dataIn := Cat(0xF.U(4.W), io.dataAccess.dataRead(3, 0))
    }
  }

  // Direct RAM access: allowing MBC7 to directly access RAM, so that it can
  // emulate EEPROM.
  val regDirectAccessBusy = RegInit(false.B)
  mbcDirectRamAccess := mbc.io.directRam.enable
  mbc.io.directRam.dataRead := io.dataAccess.dataRead
  mbc.io.directRam.done := false.B
  // Only pass a request on when the regular ROM access isn't ongoing.
  when (mbc.io.directRam.enable && !regBusy && !regDirectAccessBusy && !io.cartridge.reqStart) {
    io.dataAccess.enable := true.B
    io.dataAccess.write := mbc.io.directRam.write
    io.dataAccess.selectRom := false.B
    io.dataAccess.address := mbc.io.directRam.address
    io.dataAccess.dataWrite := mbc.io.directRam.dataWrite
    regDirectAccessBusy := true.B
    // Always stall during a direct access for simplicity.
    io.stall := true.B
  }
  when (regDirectAccessBusy) {
    io.stall := true.B
    when (io.dataAccess.valid) {
      regDirectAccessBusy := false.B
      mbc.io.directRam.done := true.B
    }
  }
}

/**
 * Max ROM: 8 MiB (23 bits)
 * Max RAM: 128 KiB (17 bits)
 */
class EmuCartridgeDataAccess extends Bundle {
  /// Pulsed high when an access should start.
  /// All output signals are only valid when enable is high.
  val enable = Output(Bool())
  /// Whether this access is a write. Only valid when `enable`
  val write = Output(Bool())
  /// 1 for ROM select, 0 for RAM. Only valid when `enable`
  val selectRom = Output(Bool())
  /// Access address. Only valid when `enable`
  val address = Output(UInt(23.W))
  /// Data write. Only valid when `enable` and `write`.
  val dataWrite = Output(UInt(8.W))
  /// Data read. Must stay valid until the next access begins.
  val dataRead = Input(UInt(8.W))
  /// Pulsed high when the previous access has completed.
  val valid = Input(Bool())
}