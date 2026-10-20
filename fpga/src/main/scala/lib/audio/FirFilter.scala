package lib.audio

import chisel3._
import chisel3.util._

object FirFilter {
  object State extends ChiselEnum {
    val idle = Value
    val compute = Value
    val output = Value
  }

  // Zeroth-order modified Bessel function of the first kind (power series).
  private def besselI0(x: Double): Double = {
    var sum = 1.0
    var term = 1.0
    var k = 1
    while (term > sum * 1e-12) {
      val t = x / (2.0 * k)
      term *= t * t
      sum += term
      k += 1
    }
    sum
  }

  private def sinc(x: Double): Double =
    if (x == 0.0) 1.0 else math.sin(math.Pi * x) / (math.Pi * x)

  /**
   * Generate coefficients for a symmetric Kaiser windowed-sinc lowpass filter.
   *
   * @param numTaps    filter length (must be odd)
   * @param cutoffNorm -6 dB cutoff frequency, normalized to the sample rate (fc / fs)
   * @param beta       Kaiser parameter
   * @param dcGain     desired DC gain
   * @param coeffBits  coefficients are quantized to signed fixed point Q1.(coeffBits-1)
   * @return quantized coefficients, i.e. round(h * 2^(coeffBits-1))
   */
  def kaiserLowpass(
    numTaps: Int,
    cutoffNorm: Double,
    beta: Double,
    dcGain: Double,
    coeffBits: Int,
  ): Seq[BigInt] = {
    require(numTaps % 2 == 1, "use an odd tap count for a symmetric filter with integer group delay")
    require(cutoffNorm > 0.0 && cutoffNorm < 0.5)
    val m = numTaps - 1
    val raw = (0 until numTaps).map { i =>
      val r = 2.0 * i / m - 1.0
      val window = besselI0(beta * math.sqrt(math.max(0.0, 1.0 - r * r))) / besselI0(beta)
      2.0 * cutoffNorm * sinc(2.0 * cutoffNorm * (i - m / 2)) * window
    }
    val scale = dcGain / raw.sum * (1L << (coeffBits - 1))
    val quantized = raw.map(c => BigInt(math.round(c * scale)))
    val maxCoeff = (BigInt(1) << (coeffBits - 1)) - 1
    require(quantized.forall(c => c.abs <= maxCoeff), "coefficient overflow; reduce dcGain or increase coeffBits")
    quantized
  }
}

/**
 * Serial symmetric FIR filter: one multiply-accumulate per clock cycle, exploiting
 * linear-phase coefficient symmetry with a pre-adder (maps to a single DSP slice).
 *
 * A new input sample may be presented (valid && ready) at most once every
 * (numTaps / 2 + 3) cycles; one output sample is produced per input sample.
 */
class FirFilter(
  dataBits: Int,
  coeffBits: Int,
  coeffs: Seq[BigInt],
) extends Module {
  val numTaps = coeffs.length
  require(numTaps % 2 == 1, "coefficients must be odd-length")
  require(
    coeffs.zipWithIndex.forall { case (c, i) => c == coeffs(numTaps - 1 - i) },
    "coefficients must be symmetric",
  )

  // Number of unique coefficients (first half plus center tap).
  val half = numTaps / 2 + 1
  private val center = numTaps / 2
  private val idxBits = log2Ceil(numTaps)
  private val accBits = dataBits + 1 + coeffBits + log2Ceil(half)

  val io = IO(new Bundle {
    val in = Flipped(Valid(SInt(dataBits.W)))
    /** Whether a new input sample can be accepted this cycle. */
    val ready = Output(Bool())
    val out = Valid(SInt(dataBits.W))
  })

  import FirFilter.State

  val samples = Mem(numTaps, SInt(dataBits.W))
  val coeffRom = VecInit(coeffs.take(half).map(_.S(coeffBits.W)))

  // Index of the newest sample; incremented after each output is computed.
  val writeIndex = RegInit(0.U(idxBits.W))
  val state = RegInit(State.idle)
  val k = RegInit(0.U(log2Ceil(half).W))
  val acc = RegInit(0.S(accBits.W))

  val outReg = RegInit(0.S(dataBits.W))
  val outValid = RegInit(false.B)
  outValid := false.B

  io.ready := state === State.idle
  io.out.bits := outReg
  io.out.valid := outValid

  // (writeIndex - offset) mod numTaps, for offset < numTaps.
  private def tapIndex(offset: UInt): UInt = {
    val diff = writeIndex +& numTaps.U - offset
    Mux(diff >= numTaps.U, diff - numTaps.U, diff)(idxBits - 1, 0)
  }

  switch (state) {
    is (State.idle) {
      when (io.in.valid) {
        samples.write(writeIndex, io.in.bits)
        k := 0.U
        acc := 0.S
        state := State.compute
      }
    }

    is (State.compute) {
      // Tap k pairs with its mirror tap (numTaps - 1 - k); the center tap stands alone.
      val x1 = samples.read(tapIndex(k))
      val x2 = samples.read(tapIndex((numTaps - 1).U - k))
      val pair = Mux(k === center.U, x1, x1 +& x2)
      acc := acc + (pair * coeffRom(k))
      when (k === center.U) {
        writeIndex := Mux(writeIndex === (numTaps - 1).U, 0.U, writeIndex + 1.U)
        state := State.output
      } .otherwise {
        k := k + 1.U
      }
    }

    is (State.output) {
      outReg := Saturate(acc >> (coeffBits - 1), dataBits)
      outValid := true.B
      state := State.idle
    }
  }
}
