package gba.cart.emu

import chisel3._
import chisel3.util._
import gba.cart.CartridgeInterface
import lib.log.Logger
import lib.mem.{MemoryInterface, PipelineMemoryInterface}

object EmulatedCartridge {
  object BackupType extends ChiselEnum {
    /// No backup
    val None = Value
    /// SRAM or FRAM, 32 KiB
    val Sram = Value
    /// Flash, 64KiB or 128KiB
    val Flash = Value
    /// Eeprom, 512B or 8KiB
    val Eeprom = Value
  }

  class Config extends Bundle {
    /// Whether the cartridge has a Z gyroscope
    val hasGyro = Bool()
    /// Whether the cartridge has an X-Y accelerometer
    val hasAccel = Bool()
    /// Whether the cartridge has a real-time clock
    val hasRtc = Bool()
    /// Whether the cartridge has rumble
    val hasRumble = Bool()
    /// Whether a GPIO controller is present
    val hasGpio = Bool()
    /// Auto-detect backup size (EEPROM only)
    val backupAutodetect = Bool()
    /// Per-type backup size
    val backupSize = UInt(1.W)
    /// Backup type
    val backupType = BackupType()
    /// Whether we're using an emulated cartridge.
    val enabled = Bool()
  }
}

class EmulatedCartridge extends Module {
  val io = IO(new Bundle {
    val config = Input(new EmulatedCartridge.Config)
    val interface = Flipped(new CartridgeInterface)
    /// Whether the cartridge interface (underlying emulator) is enabled this cycle.
    val interfaceEnable = Input(Bool())
    /// Size of the ROM, minus one
    val romSize = Input(UInt(25.W))
    /// Current gyroscope Z sample
    val imuGyroZ = Input(UInt(12.W))
    /// Current accelerometer X sample
    val imuAccelX = Input(UInt(12.W))
    /// Current accelerometer Y sample
    val imuAccelY = Input(UInt(12.W))

    /// RTC access
    val rtcDataSelect = Input(UInt(1.W))
    val rtcDataIn = Input(UInt(32.W))
    val rtcDataOut = Output(UInt(32.W))
    val rtcDataWrite = Input(Bool())

    /// External ROM memory interface, assumed synchronous.
    val rom = Flipped(new PipelineMemoryInterface(addressWidth = 24, dataWidth = 16))
    /// External backup (RAM) memory interface, assumed synchronous.
    /// Must keep read data on the bus until the next request.
    val backup = Flipped(new MemoryInterface(addressWidth = 17, dataWidth = 8))
    /// Whether the previous memory request has not yet completed by the time the GBA needs it to.
    val stall = Output(Bool())

    /// Vibration control
    val vibrate = Output(Bool())
  })
  val logger = Logger("cart.emu")

  // True if a non-ROM peripheral in the ROM chip-select (EEPROM or GPIO) is selected (based on address).
  val romPeripheralSelected = WireDefault(false.B)
  // Whether ROM nCS fell this cycle
  val romSelectNegedge = !io.interface.nCS && RegNext(io.interface.nCS)
  val readNegedge = !io.interface.nRD && RegNext(io.interface.nRD)
  val writeNegedge = !io.interface.nWR && RegNext(io.interface.nWR)

  /// Whether a cartridge access is in progress
  val regCartActive = RegInit(false.B)
  /// Whether the ROM memory interface is busy.
  val regMemRomBusy = RegInit(false.B)
  /// Whether there's a pending (non-submitted) ROM request
  val regMemRomPending = RegInit(false.B)
  /// Address of the pending ROM request
  val regMemRomPendingAddress = Reg(UInt(24.W))
  /// Last data read from ROM memory interface
  val regMemRomReadData = Reg(UInt(16.W))
  /// Whether a ram request is starting this cycle.
  val ramStart = WireDefault(false.B)

  io.rom.address := DontCare
  io.rom.enable := false.B
  io.rom.isWrite := false.B
  io.rom.dataWrite := DontCare
  io.rom.writeStrobe := DontCare
  io.backup.address := DontCare
  io.backup.enable := false.B
  io.backup.write := DontCare
  io.backup.dataWrite := DontCare
  io.backup.writeStrobe := 1.U
  io.interface.IRQ := false.B
  io.interface.ADLoIn := regMemRomReadData
  io.interface.AHiIn := DontCare
  io.interface.isEmulated := true.B
  io.stall := false.B

  when (regMemRomBusy) {
    when (io.rom.ready) {
      logger.debug(cf"ROM mem request done: data=0x${io.rom.dataRead}%x")
      regMemRomBusy := false.B
      regMemRomReadData := io.rom.dataRead
      io.interface.ADLoIn := io.rom.dataRead
    }
  }
  when (regMemRomPending) {
    io.rom.enable := true.B
    io.rom.address := regMemRomPendingAddress
    when (io.rom.ready) {
      logger.debug(cf"Submitted pending ROM request addr=0x${regMemRomPendingAddress << 1}%x")
      regMemRomPending := false.B
      regMemRomBusy := true.B
    }
  }

  when (io.interface.reqEnd) {
    // If still busy, stall if:
    //   the memory request is not ready, OR
    //   there's a pending memory request that interrupted the one that's completing now
    when (regMemRomBusy && (!io.rom.ready || regMemRomPending)) {
      io.stall := true.B
    } .otherwise {
      regCartActive := false.B
    }
  }
  // A real cartridge ROM ignores the address lines above its size, so the ROM repeats
  // ("mirrors") past its end. Classic NES Series games check for this. Sizes that aren't a
  // power of two are rounded up, like the address decoding of a real chip.
  val romHalfwordsMinusOne = io.romSize(24, 1)
  val romAddressMask = Seq(1, 2, 4, 8, 16).foldLeft(romHalfwordsMinusOne)((m, n) => m | (m >> n))
  val romReqAddress = io.interface.reqAddress & romAddressMask

  when (io.interface.reqStart && io.interfaceEnable) {
    regCartActive := true.B

    when (io.interface.reqRom) {
      logger.debug(cf"ROM request start: addr=0x${io.rom.address << 1}%x peri=${romPeripheralSelected}")
      when (!romPeripheralSelected) {
        io.rom.enable := true.B
        io.rom.address := romReqAddress
        regMemRomBusy := true.B

        when (!io.rom.ready) {
          logger.debug(cf" --> pending the ROM mem request")
          // Can't submit right now (probably due to aborted request).
          regMemRomPending := true.B
          regMemRomPendingAddress := romReqAddress
        }
      }
    } .otherwise {
      logger.debug(cf"RAM request start: addr=0x${io.interface.reqAddress(15, 0)}%x")
      ramStart := true.B
    }

    when (regCartActive && !io.interface.reqEnd) {
      logger.debug(cf" --> interrupted previous request")
    }
  }

  // Backups
  val backupSram = Module(new SramBackup)
  backupSram.io.ramEnable := false.B
  backupSram.io.ramAddress := DontCare
  backupSram.io.ramIsWrite := DontCare
  backupSram.io.ramDataWrite := DontCare
  backupSram.io.ramReqEnd := DontCare
  backupSram.io.backup.dataRead := DontCare
  backupSram.io.backup.done := DontCare
  val backupFlash = Module(new FlashBackup)
  backupFlash.io.size := io.config.backupSize
  backupFlash.io.ramEnable := false.B
  backupFlash.io.ramAddress := DontCare
  backupFlash.io.ramIsWrite := DontCare
  backupFlash.io.ramDataWrite := DontCare
  backupFlash.io.ramReqEnd := DontCare
  backupFlash.io.backup.dataRead := DontCare
  backupFlash.io.backup.done := DontCare
  val backupEeprom = Module(new EepromBackup)
  backupEeprom.io.configSize := io.config.backupSize
  backupEeprom.io.configAutodetect := io.config.backupAutodetect
  backupEeprom.io.selected := false.B
  backupEeprom.io.readPulse := DontCare
  backupEeprom.io.writePulse := DontCare
  backupEeprom.io.dataWrite := DontCare
  backupEeprom.io.reqEnd := DontCare
  backupEeprom.io.backup.dataRead := DontCare
  backupEeprom.io.backup.done := DontCare

  switch (io.config.backupType) {
    is (EmulatedCartridge.BackupType.None) {
      io.interface.AHiIn := 0xFF.U(8.W)
    }
    is (EmulatedCartridge.BackupType.Sram) {
      io.backup <> backupSram.io.backup

      backupSram.io.ramEnable := ramStart
      backupSram.io.ramAddress := io.interface.reqAddress
      backupSram.io.ramIsWrite := io.interface.reqWrite
      io.interface.AHiIn := backupSram.io.ramDataRead
      backupSram.io.ramDataWrite := io.interface.AHiOut
      backupSram.io.ramReqEnd := io.interface.reqEnd

      when (backupSram.io.stall) {
        io.stall := true.B
      }
    }
    is (EmulatedCartridge.BackupType.Flash) {
      io.backup <> backupFlash.io.backup

      backupFlash.io.ramEnable := ramStart
      backupFlash.io.ramAddress := io.interface.reqAddress
      backupFlash.io.ramIsWrite := io.interface.reqWrite
      io.interface.AHiIn := backupFlash.io.ramDataRead
      backupFlash.io.ramDataWrite := io.interface.AHiOut
      backupFlash.io.ramReqEnd := io.interface.reqEnd

      when (backupFlash.io.stall) {
        io.stall := true.B
      }
    }
    is (EmulatedCartridge.BackupType.Eeprom) {
      io.backup <> backupEeprom.io.backup

      when (backupEeprom.io.stall) {
        io.stall := true.B
      }

      // Determine if ROM bus is selecting the EEPROM.
      val addressed = WireDefault(false.B)
      when (io.romSize(24) === 0.U) {
        // <=16MiB ROM, top bit is used
        addressed := io.interface.reqAddress(23)
      } .otherwise {
        addressed := io.interface.reqAddress(23, 7).andR
      }
      when (addressed) {
        romPeripheralSelected := true.B
      }

      val selected = RegInit(false.B)
      when (romSelectNegedge && addressed) {
        // Falling edge of nCS when addressed, we're selected.
        selected := true.B
        logger.debug("Selected eeprom")
      }
      when (selected && io.interface.nCS) {
        selected := false.B
      }

      backupEeprom.io.selected := selected
      backupEeprom.io.readPulse := readNegedge
      backupEeprom.io.writePulse := writeNegedge
      backupEeprom.io.dataWrite := io.interface.ADLoOut(0)
      backupEeprom.io.reqEnd := io.interface.reqEnd
      when (selected) {
        io.interface.ADLoIn := backupEeprom.io.dataRead
      }
    }
  }

  // Tilt sensor (accelerometer), attached to SRAM bus
  val tiltSensor = Module(new TiltSensor)
  tiltSensor.io.ramEnable := false.B
  tiltSensor.io.ramAddress := io.interface.reqAddress(11, 8)
  tiltSensor.io.ramIsWrite := io.interface.reqWrite
  tiltSensor.io.ramDataWrite := io.interface.AHiOut
  tiltSensor.io.sampleX := io.imuAccelX
  tiltSensor.io.sampleY := io.imuAccelY
  when (io.config.hasAccel) {
    tiltSensor.io.ramEnable := ramStart && io.interface.reqAddress(15)
    io.interface.AHiIn := tiltSensor.io.ramDataRead
  }

  // GPIO controller (optional), with registers at 0xC4, 0xC6, 0xC8
  val gpio = Module(new Gpio)
  gpio.io.reqWrite := false.B
  gpio.io.reqAddress := DontCare
  gpio.io.dataWrite := io.interface.ADLoOut(3, 0)
  val gpioDataIn = WireDefault(VecInit.fill(4)(0.U(1.W)))
  gpio.io.pinIn := gpioDataIn.asUInt
  when (io.config.hasGpio) {
    val regAddress = Reg(UInt(2.W))
    val addressed = io.interface.reqAddress >= (0xC2 / 2).U && io.interface.reqAddress <= (0xC8 / 2).U
    when (addressed) {
      romPeripheralSelected := true.B
      when (io.interface.reqStart) {
        regAddress := io.interface.reqAddress
      }
    }
    gpio.io.reqAddress := regAddress

    val selected = RegInit(false.B)
    when (romSelectNegedge && addressed) {
      selected := true.B
      logger.debug("Selected gpio")
    }
    when (selected) {
      when (io.interface.nCS) {
        selected := false.B
      }
      gpio.io.reqWrite := writeNegedge
      io.interface.ADLoIn := gpio.io.dataRead
    }
  }

  // Rumble, connected to GPIO 3
  io.vibrate := io.config.hasRumble && gpio.io.pinOut(3) && gpio.io.pinDir(3)

  // Gyroscope, connected to GPIO 0, 1, 2
  val gyroSensor = Module(new GyroSensor)
  gyroSensor.io.sampleZ := io.imuGyroZ
  gyroSensor.io.takeSample := false.B
  gyroSensor.io.serialClock := 0.U
  when (io.config.hasGyro) {
    gyroSensor.io.takeSample := gpio.io.pinOut(0).asBool
    gyroSensor.io.serialClock := gpio.io.pinOut(1).asBool
    gpioDataIn(2) := gyroSensor.io.serialData
  }

  // RTC, connected to GPIO 0, 1, 2
  val rtc = Module(new Rtc)
  rtc.io.emuDataSelect := io.rtcDataSelect
  rtc.io.emuDataIn := io.rtcDataIn
  rtc.io.emuDataWrite := io.rtcDataWrite
  io.rtcDataOut := rtc.io.emuDataOut
  rtc.io.serialClock := gpio.io.pinOut(0).asBool
  rtc.io.serialIn := gpio.io.pinOut(1)
  rtc.io.serialSelect := gpio.io.pinOut(2).asBool
  when (io.config.hasRtc) {
    when (rtc.io.serialSelect) {
      gpioDataIn(1) := rtc.io.serialOut
    }
  }
}
