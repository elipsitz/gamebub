package gameboy.apu

import chisel3._
import chisel3.util._

class VolumeEnvelopeConfig extends Bundle {
  /** Initial volume level */
  val initialVolume = UInt(4.W)
  /** Whether sweep increases the volume (true) or decreases (false) */
  val modeIncrease = Bool()
  /** Period of the volume sweep */
  val sweepPace = UInt(3.W)
}

class VolumeEnvelope extends Module {
  val io = IO(new Bundle {
    /** Pulsed on when triggered. */
    val trigger = Input(Bool())
    /** Configuration registers */
    val config = Input(new VolumeEnvelopeConfig)
    /** Volume envelope tick pulse from frame sequencer (64 Hz) */
    val tick = Input(Bool())
    /** Whether the NRx2 register is being written */
    val write = Input(Bool())
    /** Data being written to NRx2 register */
    val writeData = Input(UInt(8.W))
    /** Output volume level */
    val out = Output(UInt(4.W))
  })

  val volume = RegInit(0.U(4.W))
  val timer = RegInit(0.U(3.W))
  io.out := volume
  // Config latched on trigger event

  val modeIncrease = RegInit(false.B)
  val sweepPace = RegInit(0.U(3.W))

  when (sweepPace =/= 0.U && io.tick) {
    val newTimer = timer - 1.U
    timer := newTimer
    when (newTimer === 0.U) {
      timer := sweepPace

      when(modeIncrease && volume < 15.U) {
        volume := volume + 1.U
      }.elsewhen(!modeIncrease && volume > 0.U) {
        volume := volume - 1.U
      }
    }
  }

  // Direct write to NRx2 during playback ("zombie" mode)
  when (io.write) {
    val newConfig = io.writeData.asTypeOf(new VolumeEnvelopeConfig)
    val prevIncrease = modeIncrease
    val prevPace = sweepPace

    modeIncrease := newConfig.modeIncrease
    sweepPace := newConfig.sweepPace
    timer := newConfig.sweepPace

    val shouldInvert = newConfig.modeIncrease ^ prevIncrease
    val shouldTick = (newConfig.sweepPace =/= 0.U) && (prevPace === 0.U)
    val isEightTick = newConfig.modeIncrease && prevIncrease && (newConfig.sweepPace === 0.U) && (prevPace === 0.U)

    val invertedVol = Mux(newConfig.modeIncrease,
      Mux(prevPace === 0.U, volume ^ 0xF.U, (14.U - volume)(3, 0)),
      (16.U - volume)(3, 0)
    )
    val postInvertVol = Mux(shouldInvert, invertedVol, volume)

    val shouldStep = Mux(shouldInvert, !newConfig.modeIncrease && shouldTick, shouldTick || isEightTick)
    val volTicked = Mux(newConfig.modeIncrease, (postInvertVol + 1.U)(3, 0), (postInvertVol - 1.U)(3, 0))

    volume := Mux(shouldStep, volTicked, postInvertVol)
  }

  // Trigger event
  when (io.trigger) {
    modeIncrease := io.config.modeIncrease
    sweepPace := io.config.sweepPace
    volume := io.config.initialVolume
    timer := io.config.sweepPace
  }
}
