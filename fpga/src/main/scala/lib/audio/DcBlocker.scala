package lib.audio

import chisel3._

/**
 * One-pole DC blocking high-pass filter:
 *   y[n] = x[n] - x[n-1] + (1 - 2^-poleShift) * y[n-1]
 */
class DcBlocker(
  inputBits: Int,
  poleShift: Int,
  outputBits: Int,
) extends Module {
  private val accBits = inputBits + poleShift + 2

  val io = IO(new Bundle {
    /** Advances the filter by one sample period. */
    val tick = Input(Bool())
    val in = Input(SInt(inputBits.W))
    val out = Output(SInt(outputBits.W))
  })

  val xPrev = RegInit(0.S(inputBits.W))
  // Accumulator in Q(inputBits + 1).(poleShift) fixed point.
  val y = RegInit(0.S(accBits.W))
  when (io.tick) {
    xPrev := io.in
    val next = ((io.in -& xPrev) << poleShift).asSInt +& (y - (y >> poleShift))
    y := next(accBits - 1, 0).asSInt
  }
  io.out := Saturate(y >> poleShift, outputBits)
}
