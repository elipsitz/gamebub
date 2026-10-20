package lib.audio

import chisel3._

/** Clamps a wide signed value into `bits` bits. */
object Saturate {
  def apply(x: SInt, bits: Int): SInt = {
    val max = ((BigInt(1) << (bits - 1)) - 1).S
    val min = (-(BigInt(1) << (bits - 1))).S
    Mux(x > max, max(bits - 1, 0).asSInt,
      Mux(x < min, min(bits - 1, 0).asSInt, x(bits - 1, 0).asSInt))
  }
}
