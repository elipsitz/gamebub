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
  val waveCounter = RegInit(0.U(12.W))
  // Wave counter counts down from this value.
  val waveCounterReload = io.wavelength << 1

  io.waveRamAddress := waveIndex(4, 1)
  val currentSample = Mux(waveIndex(0), io.waveRamDataRead(3, 0), io.waveRamDataRead(7, 4))

  when (io.pulse4Mhz) {
    val nextWaveCounter = waveCounter + 1.U
    waveCounter := nextWaveCounter
    when (nextWaveCounter === 0.U) {
      waveCounter := waveCounterReload
      waveIndex := waveIndex + 1.U
    }
  }

  when (io.trigger) {
    waveCounter := waveCounterReload
    waveIndex := 0.U
  }

  io.out := VecInit(0.U, currentSample, currentSample >> 1, currentSample >> 2)(io.volume)
  io.maxOut := VecInit(0.U, 15.U, 7.U, 3.U)(io.volume)
  io.dacEnabled := io.dacEnable
  io.channelDisable := lengthUnit.io.channelDisable
}
