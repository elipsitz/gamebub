package gameboy.apu

import chisel3._

/** Channel 3: reads samples from wave ram and plays them back */
class WaveChannel extends Module {
  val io = IO(new ChannelIO {
    val lengthConfig = Input(new LengthControlConfig(8))

    val dacEnable = Input(Bool())
    val volume = Input(UInt(2.W))
    val wavelength = Input(UInt(11.W))

    val waveRamAddress = Output(UInt(4.W))
    val waveRamDataRead = Input(UInt(8.W))
  })

  // Length control module.
  val lengthUnit = Module(new LengthControl(8))
  lengthUnit.io.trigger := io.trigger
  lengthUnit.io.config := io.lengthConfig
  lengthUnit.io.tick := io.ticks.length

  // Index within the wave RAM
  val waveIndex = RegInit(0.U(5.W))
  // Counter that advances the waveIndex. Reset on trigger.
  val waveCounter = RegInit(0.U(13.W))
  // Wave counter counts up to this value.
  val waveCounterMax = (2048.U(13.W) - io.wavelength) << 1

  // Current sample read from wave counter
  val currentSample = RegInit(0.U(4.W))
  io.waveRamAddress := waveIndex(4, 1)

  when (io.trigger) {
    waveCounter := waveCounterMax
  }

  when (io.pulse4Mhz) {
    when (waveCounter === 0.U) {
      waveIndex := waveIndex + 1.U
      waveCounter := waveCounterMax

      // TODO we're actually off by one because waveIndex is registered?
      // Maybe this should be turned into single port ram?
      currentSample := Mux(waveIndex(0), io.waveRamDataRead(7, 4), io.waveRamDataRead(3, 0))
    } .otherwise {
      waveCounter := waveCounter - 1.U
    }
  }

  io.out := VecInit(0.U, currentSample, currentSample >> 1, currentSample >> 2)(io.volume)
  io.maxOut := VecInit(0.U, 15.U, 7.U, 3.U)(io.volume)
  io.dacEnabled := io.dacEnable
  io.channelDisable := lengthUnit.io.channelDisable
}
