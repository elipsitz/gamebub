package gameboy

import chisel3._
import chisel3.util._

/** Miscellaneous 'system control' registers. */
class SystemControl(config: Gameboy.Configuration) extends Module {
  val io = IO(new PeripheralAccess {
    val clocker = Input(new Clocker)
    /** Whether we're acting as a CGB (regardless of mode) */
    val isCgb = Input(Bool())

    /** True if the boot rom is mapped. */
    val bootRomMapped = Output(Bool())
    /** True if this is a CGB and CGB mode has been enabled. */
    val cgbMode = Output(Bool())

    val vramBank = Output(UInt(1.W))
    val wramBank = Output(UInt(3.W))

    /** Pulsed when DIV overflows */
    val divOverflow = Input(Bool())
    /** Whether the CPU is in the 'STOP' state */
    val cpuStopState = Input(Bool())
    /** Whether the CPU should exit the STOP state */
    val cpuStopExit = Output(Bool())
    /** Whether we're in double-speed mode */
    val doubleSpeed = Output(Bool())
  })

  val regBootOff = RegInit(config.skipBootrom.B)
  val regCgbMode = RegInit(true.B)
  val regVramBank = RegInit(0.U(1.W))
  val regWramBank = RegInit(0.U(3.W))
  val regDoubleSpeed = RegInit(false.B)
  /** True if speed should switch upon next STOP and DIV overflow. */
  val regPrepareSpeedSwitch = RegInit(false.B)

  io.bootRomMapped := !regBootOff
  when (io.isCgb) {
    // DMG compatibility mode doesn't take effect until leaving the bootrom.
    io.cgbMode := !regBootOff || regCgbMode
  } .otherwise {
    io.cgbMode := false.B
  }
  io.vramBank := regVramBank
  io.wramBank := Mux(regWramBank === 0.U, 1.U, regWramBank)

  // Handle speed switching
  io.cpuStopExit := false.B
  when (io.clocker.enable && regPrepareSpeedSwitch && io.cpuStopState && io.divOverflow) {
    printf(cf"Speed switch -> double speed: ${!regDoubleSpeed}\n")
    io.cpuStopExit := true.B
    regDoubleSpeed := !regDoubleSpeed
    regPrepareSpeedSwitch := false.B
  }
  io.doubleSpeed := regDoubleSpeed

  // System control registers
  when (io.enabled && io.write) {
    switch (io.address) {
      // 0xFF50 - Boot Rom control
      is (0x50.U) {
        when (!regBootOff) { regBootOff := io.dataWrite(0) }
      }

      // 0xFF4C - KEY0 (CGB only)
      is (0x4C.U) {
        when (io.isCgb) {
          when (!regBootOff) {
            regCgbMode := !io.dataWrite(2)
            printf(cf"CGB mode: ${!io.dataWrite(2)}\n")
          }
        }
      }

      // 0xFF4D - KEY1 (CGB only): Prepare speed switch
      is (0x4D.U) {
        when (io.cgbMode) { regPrepareSpeedSwitch := io.dataWrite(0) }
      }

      // FF70 - SVBK (CGB Mode only): WRAM bank
      is (0x70.U) {
        when (io.cgbMode) { regWramBank := io.dataWrite(2, 0) }
      }

      // FF4F - VBK (CGB Mode only): VRAM bank
      is (0x4F.U) {
        when (io.cgbMode) { regVramBank := io.dataWrite(0) }
      }
    }
  }
  // CGB-only registers are only accessible in CGB mode (not on a DMG, and not
  // in DMG compatibility mode after the boot ROM).
  when (io.cgbMode) {
    io.valid := Seq(0x50, 0x4C, 0x70, 0x4F, 0x4D).map(io.address === _.U).reduce((a, b) => a || b)
  } .otherwise {
    io.valid := (io.address === 0x50.U)
  }
  io.dataRead := 0xFF.U
  when (io.enabled && !io.write) {
    switch (io.address) {
      is (0x50.U) { io.dataRead := Cat("b1111111".U(7.W), regBootOff) }
      is (0x70.U) { io.dataRead := Cat("b11111".U(5.W), regWramBank) }
      is (0x4F.U) { io.dataRead := Cat("b1111111".U(7.W), regVramBank) }
      is (0x4D.U) { io.dataRead := Cat(regDoubleSpeed, "b111111".U(6.W), regPrepareSpeedSwitch) }
    }
  }
}
