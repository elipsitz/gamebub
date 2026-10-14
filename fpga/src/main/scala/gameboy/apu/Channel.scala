package gameboy.apu

import chisel3._
import chisel3.util.Counter

class ChannelIO extends Bundle {
  /** Pulsed when channel is triggered by a write */
  val trigger = Input(Bool())
  /** Frame sequencer ticks */
  val ticks = Input(new FrameSequencerTicks)
  /** 4Mhz pulse (with clock enable) */
  val pulse4Mhz = Input(Bool())

  /** Output value of the channel */
  val out = Output(UInt(4.W))
  /** Volume of the channel (maximum `out`) */
  val maxOut = Output(UInt(4.W))
  /** Pulsed when the channel gets disabled (due to length expiring or frequency sweep). **/
  val channelDisable = Output(Bool())
  /** Whether the DAC is enabled **/
  val dacEnabled = Output(Bool())
}