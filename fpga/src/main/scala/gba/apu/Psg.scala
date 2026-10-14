package gba.apu

import chisel3._
import chisel3.util._
import gameboy.apu.{ChannelIO, FrameSequencer, FrequencySweepConfig, NoiseChannel, NoiseChannelConfig, PulseChannel, PulseChannelWithSweep, VolumeEnvelopeConfig}
import gba.{MMIO, MmioMap, MmioTarget}
import lib.log.Logger

/*
 * Procedural Sound Generator -- essentially the same capabilities as the Game Boy.
 *
 * Has 4 channels, each of which outputs a sample in range 0..15, corresponding to 1.0 and -1.0.
 * These get added together (if an individual channel's DAC is enabled), for a range of -4.0 to 4.0.
 * Then, each left/right channel has a volume scaler, from 1x to 8x.
 *
 * To convert an unsigned channel sample (0..15) to a signed one, we do (0xF - (2 * value)), making a range of
 * -15 to 15. With four channels, -60 to 60. With the volume scaler, -480 to 480. This is a 10-bit signed integer.
 */
class Psg extends Module {
  val io = IO(new Bundle {
    /// Global enable
    val enable = Input(Bool())

    /// MMIO access
    val mmio = new MmioTarget()

    val channelEnabled = Output(UInt(4.W))
    val volume = Input(new ApuRegisters.PsgVolume)
    val panning = Input(new ApuRegisters.PsgPanning)
    val outputLeft = Output(SInt(10.W))
    val outputRight = Output(SInt(10.W))
  })
  val logger = Logger("apu.psg", enable = io.enable)

  // General
  val regLengthEnable = RegInit(VecInit.fill(4)(false.B))
  val channelTrigger = WireDefault(VecInit.fill(4)(false.B))
  val channelEnabled = RegInit(VecInit.fill(4)(false.B))
  io.channelEnabled := channelTrigger.asUInt

  // Frame sequencer
  val frameCounter = RegInit(0.U(15.W))
  when (io.enable) {
    frameCounter := frameCounter + 1.U
  }
  val frameSequencer = Module(new FrameSequencer)
  frameSequencer.io.clockEnable := io.enable
  frameSequencer.io.divApu := frameCounter(14) // Should go from 1 -> 0, 512Hz

  // Channel 1
  val regChannel1VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val regChannel1SweepConfig = RegInit(0.U.asTypeOf(new FrequencySweepConfig))
  val regChannel1Duty = RegInit(0.U(2.W))
  val channel1 = Module(new PulseChannelWithSweep)
  channel1.io.lengthConfig.length := DontCare
  channel1.io.lengthConfig.lengthLoad := false.B
  channel1.io.lengthConfig.enabled := regLengthEnable(0)
  channel1.io.volumeConfig := regChannel1VolumeConfig
  channel1.io.wavelength := DontCare
  channel1.io.wavelengthLoad := 0.U
  channel1.io.duty := regChannel1Duty
  channel1.io.sweepConfig := regChannel1SweepConfig

  // Channel 2
  val regChannel2VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val regChannel2Duty = RegInit(0.U(2.W))
  val regChannel2Wavelength = RegInit(0.U(11.W))
  val channel2 = Module(new PulseChannel)
  channel2.io.lengthConfig.length := DontCare
  channel2.io.lengthConfig.lengthLoad := false.B
  channel2.io.lengthConfig.enabled := regLengthEnable(1)
  channel2.io.volumeConfig := regChannel2VolumeConfig
  channel2.io.wavelength := regChannel2Wavelength
  channel2.io.duty := regChannel2Duty

  // Channel 3
  val regChannel3DacEnable = RegInit(false.B)
  val regChannel3WaveRamBank = RegInit(0.U(1.W))
  val regChannel3WaveRamSize = RegInit(0.U(1.W))
  val regChannel3Volume = RegInit(0.U(2.W))
  val regChannel3VolumeForce = RegInit(false.B)
  val regChannel3Wavelength = RegInit(0.U(11.W))
  // TODO: waveram should actually be a shift register
  val waveRam = Reg(Vec(8, UInt(32.W)))
  val channel3 = Module(new WavePsgChannel)
  channel3.io.lengthConfig.length := DontCare
  channel3.io.lengthConfig.lengthLoad := false.B
  channel3.io.lengthConfig.enabled := regLengthEnable(2)
  channel3.io.dacEnable := regChannel3DacEnable
  channel3.io.volume := regChannel3Volume
  channel3.io.volumeForce := regChannel3VolumeForce
  channel3.io.waveRamBank := regChannel3WaveRamBank
  channel3.io.waveRamSize := regChannel3WaveRamSize
  channel3.io.wavelength := regChannel3Wavelength
  channel3.io.waveRamDataRead := waveRam(channel3.io.waveRamAddress)

  // Channel 4
  val regChannel4VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val regChannel4LfsrConfig = RegInit(0.U.asTypeOf(new NoiseChannelConfig))
  val channel4 = Module(new NoiseChannel)
  channel4.io.lengthConfig.length := DontCare
  channel4.io.lengthConfig.lengthLoad := false.B
  channel4.io.lengthConfig.enabled := regLengthEnable(3)
  channel4.io.volumeConfig := regChannel4VolumeConfig
  channel4.io.lfsrConfig := regChannel4LfsrConfig

  // Shared channel stuff
  val channels: Seq[ChannelIO] = Seq(channel1.io, channel2.io, channel3.io, channel4.io)
  for (i <- 0 to 3) {
    channels(i).trigger := channelTrigger(i)
    channels(i).ticks := frameSequencer.io.ticks
    channels(i).pulse4Mhz := io.enable && (frameCounter(1, 0) === 0.U)
    when (io.enable && channelTrigger(i)) { channelEnabled(i) := true.B }
    when (io.enable && channels(i).channelDisable || !channels(i).dacEnabled) { channelEnabled(i) := false.B }
  }

  io.mmio <> MmioMap.fromSeq(Seq(
    // SOUND1CNT_L / H
    0x60 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regChannel1VolumeConfig.asUInt, regChannel1Duty.asUInt, 0.U(15.W), regChannel1SweepConfig.asUInt)),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          when (mask(0)) {
            regChannel1SweepConfig := data(6, 0).asTypeOf(regChannel1SweepConfig)
          }
          when (mask(2)) {
            regChannel1Duty := data(23, 22)
            channel1.io.lengthConfig.length := data(21, 16)
            channel1.io.lengthConfig.lengthLoad := true.B
          }
          when (mask(3)) {
            regChannel1VolumeConfig := data(31, 24).asTypeOf(regChannel1VolumeConfig)
          }
        }
      })
    ),
    // SOUND1CNT_X
    0x64 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regLengthEnable(0), 0.U(14.W))),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          channel1.io.wavelength := data
          channel1.io.wavelengthLoad := mask
          when (mask(1)) {
            regLengthEnable(0) := data(14)
            channelTrigger(0) := data(15)
          }
        }
      })
    ),
    // SOUND2CNT_L
    0x68 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regChannel2VolumeConfig.asUInt, regChannel2Duty.asUInt, 0.U(6.W))),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          when (mask(0)) {
            regChannel2Duty := data(7, 6)
            channel2.io.lengthConfig.length := data(5, 0)
            channel2.io.lengthConfig.lengthLoad := true.B
          }
          when (mask(1)) {
            regChannel2VolumeConfig := data(15, 8).asTypeOf(regChannel2VolumeConfig)
          }
        }
      })
    ),
    // SOUND2CNT_H
    0x6C -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regLengthEnable(1), 0.U(14.W))),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          val newWavelength = MMIO.mask(regChannel2Wavelength, data(10, 0), mask(1, 0))
          channel2.io.wavelength := newWavelength
          regChannel2Wavelength := newWavelength
          when (mask(1)) {
            regLengthEnable(1) := data(14)
            channelTrigger(1) := data(15)
          }
        }
      })
    ),
    // SOUND3CNT_L / H
    0x70 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(
        regChannel3VolumeForce,
        regChannel3Volume,
        0.U(5.W),
        0.U(8.W),
        0.U(8.W),
        regChannel3DacEnable,
        regChannel3WaveRamBank,
        regChannel3WaveRamSize,
        0.U(5.W)
      )),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          when (mask(0)) {
            regChannel3WaveRamSize := data(5)
            regChannel3WaveRamBank := data(6)
            regChannel3DacEnable := data(7)
          }
          when (mask(2)) {
            channel3.io.lengthConfig.length := data(23, 16)
            channel3.io.lengthConfig.lengthLoad := true.B
          }
          when (mask(3)) {
            regChannel3Volume := data(30, 29)
            regChannel3VolumeForce := data(31)
          }
        }
      })
    ),
    // SOUND3CNT_X
    0x74 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regLengthEnable(2), 0.U(14.W))),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          val newWavelength = MMIO.mask(regChannel3Wavelength, data(10, 0), mask(1, 0))
          channel3.io.wavelength := newWavelength
          regChannel3Wavelength := newWavelength
          when (mask(1)) {
            regLengthEnable(2) := data(14)
            channelTrigger(2) := data(15)
          }
        }
      })
    ),
    // SOUND4CNT_L
    0x78 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regChannel4VolumeConfig.asUInt, 0.U(8.W))),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          when (mask(0)) {
            channel4.io.lengthConfig.length := data(5, 0)
            channel4.io.lengthConfig.lengthLoad := true.B
          }
          when (mask(1)) {
            regChannel4VolumeConfig := data(15, 8).asTypeOf(regChannel4VolumeConfig)
          }
        }
      })
    ),
    // SOUND4CNT_H
    0x7C -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regLengthEnable(3), 0.U(6.W), regChannel4LfsrConfig.asUInt)),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          when (mask(0)) {
            regChannel4LfsrConfig := data(7, 0).asTypeOf(regChannel4LfsrConfig)
          }
          when (mask(1)) {
            regLengthEnable(3) := data(14)
            channelTrigger(3) := data(15)
          }
        }
      })
    ),
    )
    ++
    // WAVE RAM
    // Addressed ram accesses the bank that *isn't* selected.
    (0 until 4).map(i => {
      (0x90 + (i * 4)) -> MmioMap.Entry(
        MmioMap.ReadFn(Mux(regChannel3WaveRamBank === 0.U, waveRam(4 + i), waveRam(i))),
        MmioMap.WriteFn((enable, data, mask) => {
          when (enable) {
            when (regChannel3WaveRamBank === 0.U) {
              waveRam(4 + i) := MMIO.mask(waveRam(4 + i), data, mask)
            } .otherwise {
              waveRam(i) := MMIO.mask(waveRam(i), data, mask)
            }
          }
        })
      )
    })
  )

  // Mixing

  // Convert 4-bit unsigned raw channel output to 5-bit signed
  val dacOutput = VecInit(Seq.tabulate(4)(i => {
    Mux(
      channelEnabled(i) && channels(i).dacEnabled,
      (channels(i).out << 1).zext - channels(i).maxOut.zext,
      0.S,
    )
  }))

  val mixerLeft = VecInit((0 to 3).map(i => Mux(io.panning.left(i), dacOutput(i), 0.S))).reduceTree(_ +& _)
  val mixerRight = VecInit((0 to 3).map(i => Mux(io.panning.right(i), dacOutput(i), 0.S))).reduceTree(_ +& _)
  io.outputLeft := mixerLeft * (io.volume.volumeLeft +& 1.U).zext
  io.outputRight := mixerRight * (io.volume.volumeRight +& 1.U).zext
}
