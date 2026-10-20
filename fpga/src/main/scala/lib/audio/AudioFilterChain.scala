package lib.audio

import chisel3._
import chisel3.util._

object AudioFilterChain {
  /**
   * Find the closest rational approximation (num / den) for a target ratio
   * where den <= maxDen that satisfies RateInterpolator's constraints.
   */
  def bestRational(target: Double, maxDen: Int = 1000): (Int, Int) = {
    (1 to maxDen)
      .filter(den => (den - 1) * math.round(65536.0 / den) < 65536)
      .map(den => (math.round(target * den).toInt, den))
      .filter { case (num, den) => num > 2 * den }
      .minBy { case (num, den) => math.abs(num.toDouble / den - target) }
  }
}

/**
  * Audio filter chain for converting raw sample output from cores
  * to the output DAC sample rate. Provides good quality with reasonable
  * resource consumption.
  * 
  * Runs in the system clock domain, and holds output samples continuously
  * for synchronization into the target clock domain.
  */
class AudioFilterChain(
  val numChannels: Int = 2,
  val dataBits: Int = 16,
  val inputRateHz: Int,
  val outputRateHz: Int = 48000,
  val cicR: Int,
  val cicOrder: Int = 3,
  val firTaps: Int = 75,
  val firCutoffHz: Double = 20000.0,
  val kaiserBeta: Double = 6.0,
  val dcBlockerCutoffHz: Double = 10,
  val chainDcGain: Double = 0.89,
) extends Module {
  val io = IO(new Bundle {
    val in = Input(Vec(numChannels, SInt(dataBits.W)))
    val out = Output(Vec(numChannels, SInt(dataBits.W)))
    val valid = Output(Bool())
  })

  require(
    cicR >= ((firTaps / 2) + 3),
    s"cicR ($cicR) must be >= FIR cycles (${(firTaps / 2) + 3})"
  )

  val intermediateBits = dataBits + 2
  val coeffBits = dataBits + 2
  val intermediateRateHz = inputRateHz.toDouble / cicR

  // Strobe accumulator for pacing output samples in the system clock domain.
  val (phaseNum, phaseDen) = AudioFilterChain.bestRational(intermediateRateHz / outputRateHz)
  val strobeStep = phaseDen
  val strobeModulo = phaseNum * cicR

  val firDcGain = chainDcGain / CicDecimator.gain(dataBits, order = cicOrder, cicR, intermediateBits)
  val firCoeffs = FirFilter.kaiserLowpass(
    numTaps = firTaps,
    cutoffNorm = firCutoffHz / intermediateRateHz,
    beta = kaiserBeta,
    dcGain = firDcGain,
    coeffBits = coeffBits,
  )

  // Stage 1: CIC decimation
  //
  // This brings the audio samples down from the full input sample rate (e.g. 8 MHz)
  // down to a more manageable intermediate rate (input divided by cicR),
  // for further processing.
  val cic = Seq.fill(numChannels)(
    Module(new CicDecimator(dataBits, cicR, intermediateBits, order = cicOrder))
  )
  for (i <- 0 until numChannels) {
    cic(i).io.input := io.in(i)
  }

  // Stage 2: FIR filter
  //
  // This filter, based on the coefficients, applies a low-pass filter
  // (starting at 20 kHz, brick wall at 24 kHz) to remove frequencies that
  // can't be represented in the output 48 kHz, to avoid aliasing.
  // It also corrects for droop caused by the CIC decimation.
  // 
  // This is the most computationally expensive part of the filter chain, and
  // takes ~(taps / 2) cycles per sample (hence why we needed the CIC
  // decimation stage before this).
  val fir = Seq.fill(numChannels)(
    Module(new FirFilter(intermediateBits, coeffBits, firCoeffs))
  )
  for (i <- 0 until numChannels) {
    fir(i).io.in <> cic(i).io.output
  }

  // Stage 3: Fractional-Ratio Interpolator
  //
  // This resamples the samples from the baseband rate (above) to the
  // final output rate (nominally 48 kHz). This particular implementation
  // uses Catmull-Rom cubic interpolation.
  val interpolator = Module(new RateInterpolator(numChannels, intermediateBits, phaseNum, phaseDen))
  // All FIR filters run from the same CIC valid strobe.
  interpolator.io.inValid := VecInit(fir.map(_.io.out.valid)).asUInt.andR
  for (i <- 0 until numChannels) {
    interpolator.io.in(i) := fir(i).io.out.bits
  }

  // Output rate strobe generator
  val strobeAcc = RegInit(0.U(log2Ceil(strobeModulo + strobeStep + 1).W))
  val nextStrobeAcc = strobeAcc +& strobeStep.U
  val strobeFire = nextStrobeAcc >= strobeModulo.U
  strobeAcc := Mux(strobeFire, nextStrobeAcc - strobeModulo.U, nextStrobeAcc)
  interpolator.io.outEnable := strobeFire

  // Stage 4: DC blockers
  //
  // This removes the DC offset from the output signal by applying a high-pass filter.
  val poleShift = math.round(-math.log(2 * math.Pi * dcBlockerCutoffHz / outputRateHz) / math.log(2.0)).toInt
  val dcBlocker = Seq.fill(numChannels)(
    Module(new DcBlocker(interpolator.outputBits, poleShift, dataBits))
  )
  for (i <- 0 until numChannels) {
    dcBlocker(i).io.tick := interpolator.io.outValid
    dcBlocker(i).io.in := interpolator.io.out(i)
  }

  // Stage 5: Output hold
  //
  // Hold the output stable for it to be sampled into the audio output domain.
  val regOut = Seq.fill(numChannels)(RegInit(0.S(dataBits.W)))
  for (i <- 0 until numChannels) {
    regOut(i) := dcBlocker(i).io.out
    io.out(i) := regOut(i)
  }
  io.valid := interpolator.io.outValid
}
