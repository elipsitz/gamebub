package gameboy.cart

import chisel3._

/// Physical Game Boy cartridge interface, with some additional signals to help with emulated cartridges.
class CartridgeInterface extends Bundle {
  /// Memory bus clock (1 MHz normal, 2 MHz in double-speed mode)
  val phi = Output(Bool())
  /// Write select
  val nWR = Output(Bool())
  /// Read select
  val nRD = Output(Bool())
  /// ROM chip select (1 for ROM, 0 for RAM)
  val nCS = Output(Bool())
  /// Address
  val address = Output(UInt(16.W))
  /// Data (8-bit, bidirectional)
  val dataIn = Input(UInt(8.W))
  val dataOut = Output(UInt(8.W))
  val dataDir = Output(Bool())  // 1 for output, 0 for input
  /// Reset: bidirectional open-drain, requires special handling
  /// Not used: analog audio input

  /// Non-physical signals to improve timing of emulated cartridge.

  /// Whether a new request is starting.
  val reqStart = Output(Bool())
  /// Whether this request is for ROM
  val reqRom = Output(Bool())
  /// Whether this request will be a read (0) or a write (1)
  val reqWrite = Output(Bool())
  /// The address of the request
  val reqAddress = Output(UInt(16.W))
  /// Whether the current request needs to complete by the next clock cycle.
  /// If this is high, and the request is not yet complete, the whole system
  /// should be disabled until it is ready.
  val reqEnd = Output(Bool())
  /// Data out (write) for the request
  val reqDataWrite = Output(UInt(8.W))
}
