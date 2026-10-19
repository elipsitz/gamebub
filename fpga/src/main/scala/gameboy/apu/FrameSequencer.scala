package gameboy.apu

import chisel3._
import chisel3.util.RegEnable

class FrameSequencerTicks extends Bundle {
  val volume = Bool()
  val length = Bool()
  val frequency = Bool()
}

/**
 * The frame sequencer generates low frequency pulses to advance frequency sweep,
 * volume sweep, and the length timer.
 */
class FrameSequencer extends Module {
  val io = IO(new Bundle {
    /** Clock enable */
    val clockEnable = Input(Bool())
    /** DIV-APU bit from the timer */
    val divApu = Input(Bool())
    /** Output event pulses */
    val ticks = Output(new FrameSequencerTicks())
  })

  val divPulse = RegEnable(io.divApu, false.B, io.clockEnable) && !io.divApu
  val divCounter = RegInit(0.U(3.W))
  when (divPulse && io.clockEnable) { divCounter := divCounter + 1.U }

  // Step |
  //    0 | Length
  //    1 | 
  //    2 | Length  Sweep
  //    3 | 
  //    4 | Length
  //    5 | 
  //    6 | Length  Sweep
  //    7 |                Envelope
  io.ticks.length := divPulse && io.clockEnable && (divCounter(0) === 0.U)
  io.ticks.frequency := divPulse && io.clockEnable && (divCounter(1, 0) === 2.U)
  io.ticks.volume := divPulse && io.clockEnable && (divCounter(2, 0) === 7.U)
}
