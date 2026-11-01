package gba

import chisel3._
import chisel3.util._
import gba.GameBoyPlayer.CommState
import gba.link.Link
import gba.ppu.PpuOutput
import lib.log.Logger

/// Game Boy Player support
///
/// Handles detection (based on PPU output), and communication.
/// The only "special feature" is rumble support.
class GameBoyPlayer extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())

    /// Raw PPU video output
    val ppu = Input(new PpuOutput)

    /// Override keypad input (during detection)
    val doKeypadOverride = Output(Bool())
    val keypadOverride = Output(UInt(10.W))

    /// Link port
    val link = new Link.Interface
    /// Override link input (after detected)
    val doLinkOverride = Output(Bool())

    /// Rumble enable
    val rumble = Output(Bool())
  })
  val logger = Logger("gbp", enable = io.enable)

  // Whether rumble is on
  val regRumble = RegInit(false.B)
  io.rumble := regRumble

  val vblankPulse = io.ppu.vblank && !RegEnable(io.ppu.vblank, io.enable)
  val hblankPulse = io.ppu.hblank && !RegEnable(io.ppu.hblank, io.enable)
  val scanline = RegInit(0.U(8.W))
  when (io.enable) {
    when (vblankPulse) {
      scanline := 0.U
    } .elsewhen (hblankPulse) {
      scanline := scanline + 1.U
    }
  }

  // Whether serial is active (post-detection)
  val regSerialEnabled = RegInit(false.B)
  // Serial data in and out
  val regSerialData = Reg(UInt(32.W))
  // Transfer complete and ready for processing
  val regTransferDone = RegInit(false.B)

  // Link port
  val regLinkOut = Reg(Bool())
  val regLinkClock = RegInit(true.B)
  io.link.dir := DontCare
  io.link.out.sd := false.B
  io.link.out.si := regLinkOut
  io.link.out.so := DontCare
  io.link.out.sc := regLinkClock
  io.doLinkOverride := regSerialEnabled

  handleSplashScreenDetection()
  handleSerialTransfers()
  handleCommunication()

  /// Handle physical layer serial communications
  private def handleSerialTransfers(): Unit = {
    // Do one transfer every ~15 milliseconds
    val regMainCounter = RegInit(0.U(18.W))
    val regClockTimer = Reg(UInt(6.W))
    when (io.enable && regSerialEnabled) {
      val nextCounter = regMainCounter +& 1.U
      regMainCounter := nextCounter

      // A specific tick ping happens when nextCounter at bit N is not the same as the prev counter
      // Which means that (2 ** N) ticks happened
      // At GBA clock of 16 MHz, that's [2 ** (24 - N)] Hz.
      val tickTransferStart = nextCounter(18)
      val tickClockEdge = nextCounter(2) =/= regMainCounter(2)

      when (tickTransferStart) {
        logger.debug(cf"Transfer start: send=${regSerialData}%x")
        regClockTimer := 32.U
      }
      when (regClockTimer =/= 0.U && tickClockEdge) {
        regLinkClock := !regLinkClock

        when (regLinkClock) {
          // Falling edge: output new value
          regLinkOut := regSerialData(31)
        } .otherwise {
          // Rising edge: sample value
          val nextShifter = Cat(regSerialData, io.link.in.so)
          regSerialData := nextShifter
          val nextClockTimer = regClockTimer - 1.U
          regClockTimer := nextClockTimer

          when (nextClockTimer === 0.U) {
            val response = nextShifter(31, 0)
            logger.debug(cf"Transfer complete: recv=${response}%x")
            regTransferDone := true.B
          }
        }
      }
    }
  }

  /// Handle logical serial communications
  private def handleCommunication(): Unit = {
    val regState = RegInit(CommState.Handshake)
    val regHandshakeCounter = RegInit(0.U(3.W))
    val regPrevRecvData = Reg(UInt(16.W))
    val regPrevSentData = Reg(UInt(16.W))
    val regJustSentData = Reg(UInt(16.W))

    val handshake = VecInit(Seq(0x494E, 0x544E, 0x4E45, 0x4F44).map(_.U(16.W)))
    // Consecutive bad transfers in the active state before giving up (~2 seconds).
    val maxActiveErrors = 127
    val regActiveErrors = RegInit(0.U(log2Ceil(maxActiveErrors + 1).W))
    val doReset = WireDefault(false.B)

    // Checksum after handshake phase
    val checksumCalc = VecInit((1 to 7).map(i => regSerialData(i * 4 + 3, i * 4))).reduceTree((a, b) => a ^ b)
    val checksumBad = checksumCalc =/= regSerialData(3, 0)

    when (regTransferDone) {
      regTransferDone := false.B
      logger.debug(cf"Transfer, state=${regState}")

      switch (regState) {
        is (CommState.Handshake) {
          // Receive response, must be inversion of what we last sent.
          val recvResp = regSerialData(15, 0)
          // Receive data (sent by the GBA)
          val recvData = regSerialData(31, 16)
          val nextHandshakeCounter = WireDefault(regHandshakeCounter)

          when (recvResp === ~regPrevSentData) {
            // The complement matches what we sent last transfer.

            when (regHandshakeCounter === 4.U) {
              // Handshake complete, go to Exchange1
              logger.info("Handshake complete")
              regState := CommState.Finalize1
              regTransferDone := true.B  // Allow the next stage to handle this
            } .elsewhen ((recvData === regPrevRecvData) && (recvData === regPrevSentData)) {
              // 1) GBA sent the same data as it did last time (it's stable)
              // 2) The data it sent was the data we sent (it's the handshake)
              logger.debug("Handshake advances")
              nextHandshakeCounter := regHandshakeCounter + 1.U
            }
          } .otherwise {
            // Complement is wrong, communication error.
            doReset := true.B
            nextHandshakeCounter := 0.U
          }

          // Set the next data
          val nextSendData = Wire(UInt(16.W))
          when (nextHandshakeCounter < 4.U) {
            nextSendData := handshake(nextHandshakeCounter)
          } .otherwise {
            // Send the 0x8002 code
            nextSendData := 0x8002.U(16.W)
          }
          regPrevRecvData := recvData
          regPrevSentData := regJustSentData
          regJustSentData := nextSendData
          regSerialData := Cat(~recvData, nextSendData)
          regHandshakeCounter := nextHandshakeCounter
        }
        is (CommState.Finalize1) {
          val otherData = regPrevRecvData
          // Should we confirm it's 0x8000? GBP doesn't seem to.
          logger.info(cf"Finalize 1: other_data=${otherData}%x")
          regSerialData := 0x10000010L.U
          regState := CommState.Finalize2
        }
        is (CommState.Finalize2) {
          logger.info(cf"Finalize 2: recv=${regSerialData}%x")
          when (checksumBad) {
            logger.warn("Checksum mismatch")
            doReset := true.B
          } .elsewhen (regSerialData(31, 4) =/= 0x1000001.U) {
            doReset := true.B
          } .otherwise {
            regSerialData := 0x20000013.U
            regState := CommState.Finalize3
          }
        }
        is (CommState.Finalize3) {
          logger.info(cf"Finalize 3: recv=${regSerialData}%x")
          when (checksumBad) {
            logger.warn("Checksum mismatch")
            doReset := true.B
          } .elsewhen (regSerialData(31, 4) =/= 0x2000001.U) {
            doReset := true.B
          } .otherwise {
            regSerialData := 0x30000003.U
            regState := CommState.Active
          }
        }
        is (CommState.Active) {
          // Do rumble transfer
          regSerialData := 0x30000003.U
          when (regSerialData(31, 16) === handshake(0)) {
            // The game has restarted communications (e.g. after a soft reset).
            logger.warn("Handshake restarted")
            doReset := true.B
          } .elsewhen (checksumBad || regSerialData(31, 28) =/= 0x4.U) {
            // Ignore occasional bad transfers (e.g. while the game is busy and
            // not servicing serial interrupts), rather than dropping the
            // connection, which games only establish once at startup.
            logger.warn("Bad rumble transfer")
            regActiveErrors := regActiveErrors + 1.U
            when (regActiveErrors === maxActiveErrors.U) {
              doReset := true.B
            }
          } .otherwise {
            // Handle rumble payload.
            regActiveErrors := 0.U
            val payload = regSerialData(11, 4)
            val controllers = payload.asTypeOf(Vec(4, UInt(2.W)))
            logger.info(cf"Rumble(0)=${controllers(0)}")

            switch (controllers(0)) {
              // 0: rumble off
              is (0x0.U) { regRumble := false.B }
              // 1: hard stop
              is (0x1.U) { regRumble := false.B }
              // 2: rumble on
              is (0x2.U) { regRumble := true.B }
            }
          }
        }
      }
    }

    when (doReset) {
      logger.warn("Comm failure, reset")
      regState := CommState.Handshake
      regHandshakeCounter := 0.U
      regActiveErrors := 0.U
      regRumble := false.B
    }
  }

  /// Handle the "Game Boy Player" splash screen detection.
  private def handleSplashScreenDetection(): Unit = {
    // Detection is only active during the first N frames
    val detectionFrameLimit = 7200
    val regDetectionTimeout = RegInit(0.U(log2Ceil(detectionFrameLimit).W))
    val detectionAllowed = regDetectionTimeout < detectionFrameLimit.U
    when (io.enable && vblankPulse && detectionAllowed) {
      regDetectionTimeout := regDetectionTimeout + 1.U
    }

    // Logo must be shown for N frames to enable serial communications
    val detectionFrameThreshold = 30
    val regDetectedFrames = RegInit(0.U(log2Ceil(detectionFrameThreshold).W))

    val regDetectedPrevFrame = RegInit(false.B)
    val regDetectKeypadCounter = RegInit(0.U(2.W))
    val regScanline = Reg(UInt(8.W))
    val regDetectedThisFrame = RegInit(false.B)
    val regDetectLineWhite = Reg(Bool())
    val regDetectLinePurple = Reg(Bool())
    val regDetectHash = Reg(UInt(16.W))
    val hashes = VecInit(Seq(
      0xc648, 0xfc1c, 0x4d01, 0x7128, 0xc994, 0x2f8f, 0x26ee, 0x5425, 0xe8e0,
      0xdd85, 0xe666, 0xf471, 0xd51e, 0xa1f3, 0xe597, 0xaf73, 0x4d28, 0x43e2,
      0xd912, 0xeedc, 0x6a20, 0x02de, 0xe188, 0xf14d, 0x2707, 0xba23, 0x4450,
      0x2ba6, 0xa61e, 0xd978, 0xff10, 0xff10, 0xfcfc, 0xa181, 0x8522, 0x2b94,
      0x1ba6, 0xec2c, 0x4810, 0x15c2, 0xe1d5, 0xb818, 0x3f09, 0x349e, 0x3d81,
    ).map(x => x.U(16.W)))
    when (!(io.enable && detectionAllowed)) {
      // Do not do detection
    } .elsewhen (io.ppu.vblank) {
      when (vblankPulse) {
        regScanline := 0.U
        regDetectLineWhite := true.B
        regDetectLinePurple := true.B
        regDetectHash := 0.U
        regDetectedPrevFrame := regDetectedThisFrame
        regDetectedThisFrame := true.B

        when (regDetectedThisFrame) {
          logger.info("Detected Game Boy Player screen")

          when (regDetectedFrames === (detectionFrameThreshold - 1).U) {
            logger.crit("Game Boy Player serial enabled")
            regSerialEnabled := true.B
          } .otherwise {
            regDetectedFrames := regDetectedFrames + 1.U
          }

          // Cycle through keypad pattern.
          regDetectKeypadCounter := regDetectKeypadCounter + 1.U
          when (regDetectKeypadCounter === 2.U) {
            regDetectKeypadCounter := 0.U
          }
        }
      }
    } .elsewhen (io.ppu.hblank) {
      when (hblankPulse) {
        // Check whether the scanline that just finished is valid.
        // First 56 lines are all white
        // Next 45 lines have the logo (all purpleish, with B >= R >= G)
        // Last 59 lines are all white
        val shouldBeBlank = (regScanline < 56.U) || (regScanline >= 101.U)
        when (shouldBeBlank && !regDetectLineWhite) {
          regDetectedThisFrame := false.B
        }
        when (!regDetectLinePurple) {
          regDetectedThisFrame := false.B
        }
        when (regScanline >= 56.U && regScanline < 101.U) {
          when (regDetectHash =/= hashes((regScanline - 56.U)(5, 0))) {
            regDetectedThisFrame := false.B
          }
        }

        regScanline := regScanline + 1.U
        regDetectLineWhite := true.B
        regDetectLinePurple := true.B
        regDetectHash := 0.U
      }
    } .elsewhen (io.ppu.valid) {
      val pixelR = io.ppu.pixel(4, 0)
      val pixelG = io.ppu.pixel(9, 5)
      val pixelB = io.ppu.pixel(14, 10)
      when (!(io.ppu.pixel === 0x7FFF.U)) {
        regDetectLineWhite := false.B
      }
      when (!(pixelB >= pixelR && pixelR >= pixelG)) {
        regDetectLinePurple := false.B
      }
      // Weak checksum. TODO replace with CRC
      regDetectHash := regDetectHash + io.ppu.pixel
    }

    // Output the detection keypad sequence:
    // 2 frames of no keys pressed, then 1 frame of {left, right, up, down} all pressed
    io.doKeypadOverride := regDetectedPrevFrame && detectionAllowed
    io.keypadOverride := Mux(regDetectKeypadCounter === 2.U, 0x0F0.U, 0x000.U)
  }
}

object GameBoyPlayer {
  private object CommState extends ChiselEnum {
    /// "NINTENDO" handshake
    val Handshake = Value

    val Finalize1 = Value
    val Finalize2 = Value
    val Finalize3 = Value

    /// Rumble back and forth
    val Active = Value
  }
}