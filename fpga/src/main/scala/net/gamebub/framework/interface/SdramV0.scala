package net.gamebub.framework.interface

import chisel3._

object SdramV0 {
  /** The number of SDRAM chips available on this hardware. */
  var numChips: Int = 0
}

class SdramV0(
    /** Physical address width */
    val addressWidth: Int = 13,
    /** Physical data width (word size) */
    val dataWidth: Int = 16,
    /** Bank address width */
    val bankWidth: Int = 2,
    /** Number of (parallel) chips used */
    val chips: Int = 1,
) extends Bundle {
  /** Clock */
  val clock = Output(Clock())
  /** Clock Enable */
  val cke = Output(UInt(chips.W))

  /** Chip Select (active-low) */
  val cs = Output(UInt(chips.W))
  /** Row Address Strobe (active-low) */
  val ras = Output(Bool())
  /** Column Address Strobe (active-low) */
  val cas = Output(Bool())
  /** Write Enable (active-low) */
  val we = Output(Bool())

  /** Data Mask (byte) */
  val dqm = Output(UInt((dataWidth / 8).W))
  /** Bank Select */
  val bank = Output(UInt(bankWidth.W))
  /** Address */
  val address = Output(UInt(addressWidth.W))
  /** Data Input */
  val dataIn = Input(UInt(dataWidth.W))
  /** Data Output */
  val dataOut = Output(UInt(dataWidth.W))
  /** Data Direction: true for output. */
  val dataDir = Output(Bool())
}
