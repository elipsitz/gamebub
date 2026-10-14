package gba.apu

import chisel3._
import chisel3.util._
import gameboy.apu.{ChannelIO, LengthControl, LengthControlConfig}

/**
 * Channel 3: reads samples from wave ram and plays them back
 *
 * Changes in GBA: supports banked wave RAM (2x the size)
 */
class WavePsgChannel extends Module {
  val io = IO(new ChannelIO {
    val lengthConfig = Input(new LengthControlConfig(8))

    val dacEnable = Input(Bool())
    val volume = Input(UInt(2.W))
    /// Force 75% volume
    val volumeForce = Input(Bool())
    val wavelength = Input(UInt(11.W))

    /// Which bank to play from
    val waveRamBank = Input(UInt(1.W))
    /// 0: one bank, 1: two banks
    val waveRamSize = Input(UInt(1.W))
    // TODO: should actually be a big shift register
    val waveRamAddress = Output(UInt(3.W))
    val waveRamDataRead = Input(UInt(32.W))
  })

  // Length control module.
  val lengthUnit = Module(new LengthControl(8))
  lengthUnit.io.trigger := io.trigger
  lengthUnit.io.config := io.lengthConfig
  lengthUnit.io.tick := io.ticks.length

  // Index within the wave RAM
  val waveIndex = RegInit(0.U(6.W))
  // Counter that advances the waveIndex. Reset on trigger.
  val waveCounter = RegInit(0.U(13.W))
  // Wave counter counts up to this value.
  val waveCounterMax = (2048.U(13.W) - io.wavelength) << 1

  // Current sample read from wave counter
  val currentSample = RegInit(0.U(4.W))
  io.waveRamAddress := waveIndex(5, 3)

  when (io.trigger) {
    waveCounter := waveCounterMax
    waveIndex := Cat(io.waveRamBank, 0.U(5.W))
  }

  when (io.pulse4Mhz) {
    when (waveCounter === 0.U) {
      val nextIndex = waveIndex + 1.U
      when (io.waveRamSize === 0.U) {
        // Loop a single bank
        waveIndex := Cat(io.waveRamBank, nextIndex(4, 0))
      } .otherwise {
        // Loop both banks
        waveIndex := nextIndex
      }

      waveCounter := waveCounterMax

      val sampleByte = io.waveRamDataRead.asTypeOf(Vec(4, UInt(8.W)))(waveIndex(2, 1))
      val sampleNibble = Mux(waveIndex(0), sampleByte(3, 0), sampleByte(7, 4))
      currentSample := sampleNibble
    } .otherwise {
      waveCounter := waveCounter - 1.U
    }
  }

  io.out := Mux(
    io.volumeForce,
    (currentSample >> 1).asUInt +& (currentSample >> 2).asUInt,
    VecInit(0.U, currentSample, currentSample >> 1, currentSample >> 2)(io.volume)
  )
  io.maxOut := Mux(
    io.volumeForce,
    10.U,
    VecInit(0.U, 15.U, 7.U, 3.U)(io.volume)
  )
  io.dacEnabled := io.dacEnable
  io.channelDisable := lengthUnit.io.channelDisable
}
