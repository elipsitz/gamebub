package gameboy.apu

import chisel3._


class LengthControlConfig(bits: Int) extends Bundle {
  /** Raw length input (inverted + 1 when loaded) */
  val length = UInt(bits.W)
  /** Whether length is being loaded this cycle */
  val lengthLoad = Bool()
  /** Whether the length control is enabled */
  val enabled = Bool()
}

class LengthControl(bits: Int) extends Module {
  val io = IO(new Bundle {
    /** Pulsed on when triggered. */
    val trigger = Input(Bool())
    /** Configuration registers */
    val config = Input(new LengthControlConfig(bits))
    /** Length control tick pulse from frame sequencer (256 Hz) */
    val tick = Input(Bool())
    /** Pulsed when the channel output should get disabled */
    val channelDisable = Output(Bool())
  })

  val maxLength = (1 << bits).U
  val timer = RegInit(0.U((bits + 1).W))
  io.channelDisable := false.B

  when (io.trigger) {
    when (timer === 0.U) {
      timer := maxLength
    }
  } .elsewhen (io.tick && io.config.enabled && timer =/= 0.U) {
    val newTimer = timer - 1.U
    timer := newTimer
    when (newTimer === 0.U) {
      // Channel only gets disabled if we *tick down* to 0.
      io.channelDisable := true.B
    }
  }

  when (io.config.lengthLoad) {
    timer := maxLength - io.config.length
  }
}
