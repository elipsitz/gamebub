package lib.audio

import chisel3._
import chisel3.util._

object RateInterpolator {
  object State extends ChiselEnum {
    val idle = Value
    val consume = Value
    val load = Value
    val mul = Value
    val output = Value
  }
}

/**
 * Fractional-rate audio resampler using Catmull-Rom cubic interpolation.
 *
 * Paced by `outEnable` pulses, it consumes samples from an internal ring buffer
 * at rate ratio (phaseNum / phaseDen).
 *
 * Catmull-Rom cubic interpolation evaluates:
 *   p(t) = s1 + (t / 2) * (c1 + t * (c2 + t * c3))
 * where:
 *   c1 = s2 - s0
 *   c2 = 2*s0 - 5*s1 + 4*s2 - s3
 *   c3 = -s0 + 3*(s1 - s2) + s3
 *
 * To save FPGA DSP resources, polynomial evaluation runs serially over 3 cycles
 * using a single hardware multiplier per channel (Horner's rule).
 */
class RateInterpolator(
  numChannels: Int,
  dataBits: Int,
  phaseNum: Int,
  phaseDen: Int,
) extends Module {
  import RateInterpolator.State

  require(phaseNum > 2 * phaseDen, "must be oversampled input (ratio > 2)")
  require(phaseDen >= 1 && (phaseDen - 1) * math.round(65536.0 / phaseDen) < 65536,
    "phaseDen too large for Q0.16 fraction")

  val outputBits = dataBits + 1

  val io = IO(new Bundle {
    val inValid = Input(Bool())
    val in = Input(Vec(numChannels, SInt(dataBits.W)))
    val outEnable = Input(Bool())
    val outValid = Output(Bool())
    val out = Output(Vec(numChannels, SInt(outputBits.W)))
  })

  // Ring Buffer
  private val windowGuard = 4
  private val ceilRatio = (phaseNum + phaseDen - 1) / phaseDen
  private val prefill = windowGuard + ceilRatio + 2
  private val ringBits = math.max(4, log2Ceil(prefill + ceilRatio + 1))
  private val ringDepth = 1 << ringBits
  private val tRecip = math.round(65536.0 / phaseDen).toInt
  private val accBits = log2Ceil(phaseNum + phaseDen) + 1

  val rings = Seq.fill(numChannels)(Mem(ringDepth, SInt(dataBits.W)))
  val writePtr = RegInit(0.U(ringBits.W))
  val readPtr = RegInit(0.U(ringBits.W))
  val occupancy = RegInit(0.U((ringBits + 1).W))

  val doWrite = io.inValid && occupancy < ringDepth.U
  val consumeFire = WireDefault(false.B)
  val underrun = WireDefault(false.B)
  val overrun = io.inValid && occupancy === ringDepth.U

  when (doWrite) {
    for (ch <- 0 until numChannels) {
      rings(ch).write(writePtr, io.in(ch))
    }
    writePtr := writePtr + 1.U
  }
  occupancy := (occupancy +& doWrite.asUInt - consumeFire.asUInt)(ringBits, 0)

  // Interpolation state machine
  val state = RegInit(State.idle)
  val started = RegInit(false.B)
  val step = RegInit(0.U(2.W))

  val acc = RegInit(0.U(accBits.W))
  val tFrac = RegInit(0.U(16.W))
  val window = Seq.fill(numChannels)(Reg(Vec(4, SInt(dataBits.W))))
  val horner = Seq.fill(numChannels)(RegInit(0.S((dataBits + 6).W)))

  // Catmull-Rom polynomial coefficients: c3 (step 0), c2 (step 1), c1 (step 2)
  private def catmullCoeffs(s: Vec[SInt]): Seq[SInt] = {
    val c1 = s(2) -& s(0)
    val c2 = (s(0) << 1).asSInt - (s(1) * 5.S) + (s(2) << 2).asSInt - s(3)
    val c3 = -s(0) + ((s(1) - s(2)) * 3.S) + s(3)
    Seq(c3, c2, c1)
  }
  private val coeffs = window.map(w => VecInit(catmullCoeffs(w)))
  private val tSigned = tFrac.zext

  // Single MAC step: product = (acc + c) * t >> 16
  private def hornerStep(current: SInt, c: SInt): SInt = {
    val product = (current +& c) * tSigned
    (product >> 16)(dataBits + 5, 0).asSInt
  }

  val outReg = RegInit(VecInit(Seq.fill(numChannels)(0.S(outputBits.W))))
  val outValidReg = RegInit(false.B)
  outValidReg := false.B

  switch (state) {
    is (State.idle) {
      when (io.outEnable && (started || occupancy >= prefill.U)) {
        started := true.B
        acc := acc + phaseNum.U
        state := State.consume
      }
    }

    // Step read pointer forward until phase accumulator is in [0, phaseDen)
    is (State.consume) {
      when (acc >= phaseDen.U && occupancy > windowGuard.U) {
        acc := acc - phaseDen.U
        readPtr := readPtr + 1.U
        consumeFire := true.B
      } .elsewhen (acc >= phaseDen.U) {
        underrun := true.B
      } .otherwise {
        tFrac := (acc * tRecip.U)(15, 0)
        step := 0.U
        state := State.load
      }
    }

    // Read the 4-sample window s0..s3 sequentially from single-port Mem
    is (State.load) {
      for (ch <- 0 until numChannels) {
        window(ch)(step) := rings(ch).read(readPtr + step)
      }
      step := step + 1.U
      when (step === 3.U) {
        step := 0.U
        state := State.mul
      }
    }

    // Evaluate Horner's rule over 3 cycles: c3 -> c2 -> c1
    is (State.mul) {
      for (ch <- 0 until numChannels) {
        val prev = Mux(step === 0.U, 0.S, horner(ch))
        horner(ch) := hornerStep(prev, coeffs(ch)(step))
      }
      step := step + 1.U
      when (step === 2.U) {
        state := State.output
      }
    }

    // Final term: s1 + (horner / 2)
    is (State.output) {
      for (ch <- 0 until numChannels) {
        outReg(ch) := Saturate(window(ch)(1) +& (horner(ch) >> 1), outputBits)
      }
      outValidReg := true.B
      state := State.idle
    }
  }

  // Error recovery
  when (underrun || overrun) {
    started := false.B
    acc := 0.U
    occupancy := 0.U
    readPtr := Mux(doWrite, writePtr + 1.U, writePtr)
    state := State.idle
  }

  io.out := outReg
  io.outValid := outValidReg
}
