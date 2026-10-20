package gameboy.apu

import chisel3._
import chisel3.util._
import gameboy.{Clocker, Gameboy, PeripheralAccess}

class ApuOutput extends Bundle {
  /** Left sample value */
  val left = Output(SInt(10.W))
  /** Right sample value */
  val right = Output(SInt(10.W))
}

/**
 * Audio Processing Unit
 *
 * Has 4 channels, each of which outputs a sample in range 0..15, corresponding to 1.0 and -1.0.
 * These get added together (if an individual channel's DAC is enabled), for a range of -4.0 to 4.0.
 * Then, each left/right channel has a volume scaler, from 1x to 8x.
 *
 * To convert an unsigned channel sample (0..15) to a signed one, we do (0xF - (2 * value)), making a range of
 * -15 to 15. With four channels, -60 to 60. With the volume scaler, -480 to 480. This is a 10-bit signed integer.
 */
class Apu(config: Gameboy.Configuration) extends Module {
  val io = IO(new Bundle {
    val clocker = Input(new Clocker)
    val cgbMode = Input(Bool())

    val output = new ApuOutput
    val reg = new PeripheralAccess
    val divApu = Input(Bool())
  })

  // General registers
  val regApuEnable = RegInit(config.skipBootrom.B)
  val regPanning = RegInit(0.U.asTypeOf(new RegisterSoundPanning))
  val regVolume = RegInit(0.U.asTypeOf(new RegisterMasterVolume))
  val regLengthEnable = RegInit(VecInit.fill(4)(false.B))
  val channelTrigger = VecInit.fill(4)(false.B)
  val channelEnabled = RegInit(VecInit.fill(4)(false.B))

  // Wave ram
  val waveRam = RegInit(VecInit.fill(16)(0.U(8.W)))

  // Frame sequencer
  val frameSequencer = Module(new FrameSequencer)
  frameSequencer.io.clockEnable := io.clocker.enable
  frameSequencer.io.divApu := io.divApu

  // Channel 1
  val channel1VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val channel1SweepConfig = RegInit(0.U.asTypeOf(new FrequencySweepConfig))
  val channel1Duty = RegInit(0.U(2.W))
  val channel1 = Module(new PulseChannelWithSweep)
  channel1.io.lengthConfig.length := DontCare
  channel1.io.lengthConfig.lengthLoad := false.B
  channel1.io.lengthConfig.enabled := regLengthEnable(0)
  channel1.io.volumeConfig := channel1VolumeConfig
  channel1.io.volumeWrite := false.B
  channel1.io.volumeWriteData := DontCare
  channel1.io.wavelength := DontCare
  channel1.io.wavelengthLoad := 0.U
  channel1.io.duty := channel1Duty
  channel1.io.sweepConfig := channel1SweepConfig

  // Channel 2
  val channel2VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val channel2Duty = RegInit(0.U(2.W))
  val channel2Wavelength = RegInit(0.U(11.W))
  val channel2 = Module(new PulseChannel)
  channel2.io.lengthConfig.length := DontCare
  channel2.io.lengthConfig.lengthLoad := false.B
  channel2.io.lengthConfig.enabled := regLengthEnable(1)
  channel2.io.volumeConfig := channel2VolumeConfig
  channel2.io.volumeWrite := false.B
  channel2.io.volumeWriteData := DontCare
  channel2.io.wavelength := channel2Wavelength
  channel2.io.duty := channel2Duty

  // Channel 3
  val channel3DacEnable = RegInit(false.B)
  val channel3Volume = RegInit(0.U(2.W))
  val channel3Wavelength = RegInit(0.U(11.W))
  val channel3 = Module(new WaveChannel)
  channel3.io.lengthConfig.length := DontCare
  channel3.io.lengthConfig.lengthLoad := false.B
  channel3.io.lengthConfig.enabled := regLengthEnable(2)
  channel3.io.dacEnable := channel3DacEnable
  channel3.io.volume := channel3Volume
  channel3.io.wavelength := channel3Wavelength
  // TODO deal with wave ram conflicts?
  channel3.io.waveRamDataRead := waveRam(channel3.io.waveRamAddress)

  // Channel 4
  val channel4VolumeConfig = RegInit(0.U.asTypeOf(new VolumeEnvelopeConfig))
  val channel4LfsrConfig = RegInit(0.U.asTypeOf(new NoiseChannelConfig))
  val channel4 = Module(new NoiseChannel)
  channel4.io.lengthConfig.length := DontCare
  channel4.io.lengthConfig.lengthLoad := false.B
  channel4.io.lengthConfig.enabled := regLengthEnable(3)
  channel4.io.volumeConfig := channel4VolumeConfig
  channel4.io.volumeWrite := false.B
  channel4.io.volumeWriteData := DontCare
  channel4.io.lfsrConfig := channel4LfsrConfig

  // Shared channel stuff
  val channels: Seq[ChannelIO] = Seq(channel1.io, channel2.io, channel3.io, channel4.io)
  for (i <- 0 to 3) {
    channels(i).trigger := channelTrigger(i)
    channels(i).ticks := frameSequencer.io.ticks
    channels(i).pulse4Mhz := io.clocker.pulse4Mhz
    when (io.clocker.enable && channelTrigger(i)) { channelEnabled(i) := true.B }
    when (io.clocker.enable && channels(i).channelDisable || !channels(i).dacEnabled) { channelEnabled(i) := false.B }
  }

  // Register memory mapping
  io.reg.valid := false.B
  io.reg.dataRead := DontCare
  when (io.reg.enabled && io.reg.write) {
    switch (io.reg.address) {
      // Global registers
      is (0x26.U) { regApuEnable := io.reg.dataWrite(7) }
      is (0x25.U) { regPanning := io.reg.dataWrite.asTypeOf(new RegisterSoundPanning) }
      is (0x24.U) { regVolume := io.reg.dataWrite.asTypeOf(new RegisterMasterVolume) }

      // Channel 1 Registers
      is (0x10.U) { channel1SweepConfig := io.reg.dataWrite.asTypeOf(new FrequencySweepConfig) }
      is (0x11.U) {
        channel1Duty := io.reg.dataWrite(7, 6)
        channel1.io.lengthConfig.length := io.reg.dataWrite(5, 0)
        channel1.io.lengthConfig.lengthLoad := true.B
      }
      is (0x12.U) {
        channel1VolumeConfig := io.reg.dataWrite.asTypeOf(new VolumeEnvelopeConfig)
        channel1.io.volumeWrite := true.B
        channel1.io.volumeWriteData := io.reg.dataWrite
      }
      is (0x13.U) {
        channel1.io.wavelength := Cat(io.reg.dataWrite, io.reg.dataWrite)
        channel1.io.wavelengthLoad := "b01".U(2.W)
      }
      is (0x14.U) {
        channel1.io.wavelength := Cat(io.reg.dataWrite, io.reg.dataWrite)
        channel1.io.wavelengthLoad := "b10".U(2.W)

        regLengthEnable(0) := io.reg.dataWrite(6)
        when (io.reg.dataWrite(7)) { channelTrigger(0) := true.B }
      }

      // Channel 2 Registers
      is (0x16.U) {
        channel2Duty := io.reg.dataWrite(7, 6)
        channel2.io.lengthConfig.length := io.reg.dataWrite(5, 0)
        channel2.io.lengthConfig.lengthLoad := true.B
      }
      is (0x17.U) {
        channel2VolumeConfig := io.reg.dataWrite.asTypeOf(new VolumeEnvelopeConfig)
        channel2.io.volumeWrite := true.B
        channel2.io.volumeWriteData := io.reg.dataWrite
      }
      is (0x18.U) { channel2Wavelength := Cat(channel2Wavelength(10, 8), io.reg.dataWrite) }
      is (0x19.U) {
        regLengthEnable(1) := io.reg.dataWrite(6)
        val newWavelength = Cat(io.reg.dataWrite(2, 0), channel2Wavelength(7, 0))
        channel2Wavelength := newWavelength
        channel2.io.wavelength := newWavelength
        when (io.reg.dataWrite(7)) { channelTrigger(1) := true.B }
      }

      // Channel 3 Registers
      is (0x1A.U) { channel3DacEnable := io.reg.dataWrite(7) }
      is (0x1B.U) {
        channel3.io.lengthConfig.length := io.reg.dataWrite
        channel3.io.lengthConfig.lengthLoad := true.B
      }
      is (0x1C.U) { channel3Volume := io.reg.dataWrite(6, 5) }
      is (0x1D.U) { channel3Wavelength := Cat(channel3Wavelength(10, 8), io.reg.dataWrite) }
      is (0x1E.U) {
        regLengthEnable(2) := io.reg.dataWrite(6)
        val newWavelength = Cat(io.reg.dataWrite(2, 0), channel3Wavelength(7, 0))
        channel3Wavelength := newWavelength
        channel3.io.wavelength := newWavelength
        when (io.reg.dataWrite(7)) { channelTrigger(2) := true.B }
      }

      // Channel 4 Registers
      is (0x20.U) {
        channel4.io.lengthConfig.length := io.reg.dataWrite(5, 0)
        channel4.io.lengthConfig.lengthLoad := true.B
      }
      is (0x21.U) {
        channel4VolumeConfig := io.reg.dataWrite.asTypeOf(new VolumeEnvelopeConfig)
        channel4.io.volumeWrite := true.B
        channel4.io.volumeWriteData := io.reg.dataWrite
      }
      is (0x22.U) { channel4LfsrConfig := io.reg.dataWrite.asTypeOf(new NoiseChannelConfig) }
      is (0x23.U) {
        regLengthEnable(3) := io.reg.dataWrite(6)
        when (io.reg.dataWrite(7)) { channelTrigger(3) := true.B }
      }
    }

    // Wave RAM -- TODO, handle conflicts?
    when (io.reg.address >= 0x30.U && io.reg.address < 0x40.U) {
      waveRam(io.reg.address(3, 0)) := io.reg.dataWrite
    }
  }
  io.reg.dataRead := 0xFF.U
  when (io.reg.enabled && !io.reg.write) {
    switch (io.reg.address) {
      // Global registers
      is (0x26.U) {
        io.reg.dataRead := Cat(regApuEnable, "b111".U(3.W), channelEnabled.asUInt)
      }
      is (0x25.U) { io.reg.dataRead := regPanning.asUInt }
      is (0x24.U) { io.reg.dataRead := regVolume.asUInt }

      // Channel 1 registers
      is (0x10.U) { io.reg.dataRead := Cat("b1".U(1.W), channel1SweepConfig.asUInt) }
      is (0x11.U) { io.reg.dataRead := Cat(channel1Duty, "b111111".U(6.W)) }
      is (0x12.U) { io.reg.dataRead := channel1VolumeConfig.asUInt }
      is (0x14.U) { io.reg.dataRead := Cat("b1".U(1.W), regLengthEnable(0), "b111111".U(6.W)) }

      // Channel 2 registers
      is (0x16.U) { io.reg.dataRead := Cat(channel2Duty, "b111111".U(6.W)) }
      is (0x17.U) { io.reg.dataRead := channel2VolumeConfig.asUInt }
      is (0x19.U) { io.reg.dataRead := Cat("b1".U(1.W), regLengthEnable(1), "b111111".U(6.W)) }

      // Channel 3 Registers
      is (0x1A.U) { io.reg.dataRead := Cat(channel3DacEnable, "b1111111".U(7.W)) }
      is (0x1C.U) { io.reg.dataRead := Cat("b1".U(1.W), channel3.io.volume, "b11111".U(5.W)) }
      is (0x1E.U) { io.reg.dataRead := Cat("b1".U(1.W), regLengthEnable(2), "b111111".U(6.W)) }

      // Channel 4 Registers
      is (0x21.U) { io.reg.dataRead := channel4VolumeConfig.asUInt }
      is (0x22.U) { io.reg.dataRead := channel4LfsrConfig.asUInt }
      is (0x23.U) { io.reg.dataRead := Cat("b1".U(1.W), regLengthEnable(3), "b111111".U(6.W)) }

      // CGB-only digital output registers
      is (0x76.U) { io.reg.dataRead := Cat(channel2.io.out, channel1.io.out) }
      is (0x77.U) { io.reg.dataRead := Cat(channel4.io.out, channel3.io.out) }
    }

    when(io.reg.address >= 0x30.U && io.reg.address < 0x40.U) {
      io.reg.dataRead := waveRam(io.reg.address(3, 0))
    }
  }
  io.reg.valid := io.reg.address >= 0x10.U && io.reg.address <= 0x3F.U ||
    (io.cgbMode && (io.reg.address === 0x76.U || io.reg.address === 0x77.U))

  // APU off reset
  when (!regApuEnable && io.clocker.enable) {
    channelEnabled := VecInit.fill(4)(false.B)
    regPanning := 0.U.asTypeOf(new RegisterSoundPanning)
    regVolume := 0.U.asTypeOf(new RegisterMasterVolume)
    regLengthEnable := 0.U.asTypeOf(regLengthEnable)
    frameSequencer.reset := true.B
    // Wave ram is not reset

    channel1VolumeConfig := 0.U.asTypeOf(new VolumeEnvelopeConfig)
    channel1SweepConfig := 0.U.asTypeOf(new FrequencySweepConfig)
    channel1Duty := 0.U
    channel1.io.wavelength := 0.U
    channel1.io.wavelengthLoad := "b11".U(2.W)
    channel1.reset := true.B

    channel2VolumeConfig := 0.U.asTypeOf(new VolumeEnvelopeConfig)
    channel2Duty := 0.U
    channel2Wavelength := 0.U
    channel2.reset := true.B

    channel3DacEnable := false.B
    channel3Volume := 0.U
    channel3Wavelength := 0.U
    channel3.reset := true.B

    channel4VolumeConfig := 0.U.asTypeOf(new VolumeEnvelopeConfig)
    channel4LfsrConfig := 0.U.asTypeOf(new NoiseChannelConfig)
    channel4.reset := true.B

    // DMG: length counters are not affected by poweroff
  }

  // Mixer
  val dacOutput = VecInit(Seq.tabulate(4)(i => {
    Mux(
      channelEnabled(i) && channels(i).dacEnabled,
      (channels(i).out << 1).zext - channels(i).maxOut.zext,
      0.S,
    )
  }))
  val mixerLeft = VecInit((0 to 3).map(i => Mux(regPanning.left(i), dacOutput(i), 0.S))).reduceTree(_ +& _)
  val mixerRight = VecInit((0 to 3).map(i => Mux(regPanning.right(i), dacOutput(i), 0.S))).reduceTree(_ +& _)
  io.output.left := mixerLeft * (regVolume.leftVolume +& 1.U).zext
  io.output.right := mixerRight * (regVolume.rightVolume +& 1.U).zext
}
