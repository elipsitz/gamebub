package gameboy.apu

import chisel3._
import chisel3.util._

class NoiseChannelConfig extends Bundle {
  /** Clock shift "s" */
  val shift = UInt(4.W)
  /** LFSR width: 0 -> 15 bits, 1 -> 7 bits */
  val size = Bool()
  // Clock divider "r"
  val divider = UInt(3.W)
}

/** Channel 4: Uses LFSR to produce pseudorandom noise */
class NoiseChannel extends Module {
  val io = IO(new ChannelIO {
    val lengthConfig = Input(new LengthControlConfig(6))
    val volumeConfig = Input(new VolumeEnvelopeConfig)

    val lfsrConfig = Input(new NoiseChannelConfig)
  })

  // Length control module.
  val lengthUnit = Module(new LengthControl(6))
  lengthUnit.io.trigger := io.trigger
  lengthUnit.io.config := io.lengthConfig
  lengthUnit.io.tick := io.ticks.length

  // Volume control module.
  val volumeUnit = Module(new VolumeEnvelope)
  volumeUnit.io.trigger := io.trigger
  volumeUnit.io.config := io.volumeConfig
  volumeUnit.io.tick := io.ticks.volume

  // LFSR
  val timer = RegInit(0.U(22.W))
  val timerMax = Mux(io.lfsrConfig.divider === 0.U, 8.U, io.lfsrConfig.divider << 4) << io.lfsrConfig.shift
  val shiftRegister = RegInit(0.U(15.W))

  when (io.pulse4Mhz) {
    when (timer === 0.U) {
      timer := timerMax
      val bit = !(shiftRegister(0) ^ shiftRegister(1))
      shiftRegister := Cat(
        bit,
        shiftRegister(14, 8),
        Mux(io.lfsrConfig.size, bit, shiftRegister(7)),
        shiftRegister(6, 1),
      )
    } .otherwise {
      timer := timer - 1.U
    }
  }

  when (io.trigger) {
    shiftRegister := 0.U
    timer := timerMax
  }

  io.out := Mux(shiftRegister(0), volumeUnit.io.out, 0.U)
  io.maxOut := volumeUnit.io.out
  io.dacEnabled := io.volumeConfig.initialVolume =/= 0.U || io.volumeConfig.modeIncrease
  io.channelDisable := lengthUnit.io.channelDisable
}
