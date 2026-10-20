package gba

import chisel3._
import chisel3.util._
import lib.log.Logger

class Keypad extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val state = Input(new Keypad.State())
    val mmio = new MmioTarget()
    val irq = Output(Bool())
  })
  val logger = Logger("keypad", enable = io.enable)

  val regKeyControl = RegInit(0.U.asTypeOf(new Keypad.KeyControl))

  val keyInput = ~io.state.asUInt
  io.mmio <> MmioMap(
    // KEYINPUT and KEYCNT
    0x130 -> MmioMap.Entry(
        MmioMap.ReadFn(keyInput.pad(16), regKeyControl),
        MmioMap.WriteFn((enable, data, mask) => {
          when (enable) {
            val value = MMIO.mask(regKeyControl, data(31, 16), mask(3, 2))
            regKeyControl := value
          }
        })
    )
  )

  regKeyControl._padding := 0.U

  // Interrupt
  val mask = regKeyControl.mask.asUInt
  val selectedPressed = mask & io.state.asUInt
  val asserted = Wire(Bool())
  when (regKeyControl.irqConditionAnd) {
    asserted := selectedPressed === mask
  } .otherwise {
    asserted := selectedPressed =/= 0.U
  }
  val assertedPrev = RegEnable(asserted, io.enable)
  val statePrev = RegEnable(io.state, io.enable)
  val stateChanged = io.state.asUInt =/= statePrev.asUInt
  val maskPrev = RegEnable(mask, io.enable)
  val maskAdded = (mask & (~maskPrev).asUInt).orR
  // Actual IRQ signal has some edge detection, but it's a bit weird relating to how the keyControl mask changes.
  io.irq := regKeyControl.irqEnable && asserted && (!assertedPrev || stateChanged || maskAdded)
}

object Keypad {
  class State extends Bundle {
    val l = Bool()
    val r = Bool()
    val down = Bool()
    val up = Bool()
    val left = Bool()
    val right = Bool()
    val start = Bool()
    val select = Bool()
    val b = Bool()
    val a = Bool()
  }

  class KeyControl extends Bundle {
    val irqConditionAnd = Bool()
    val irqEnable = Bool()
    val _padding = UInt(4.W)
    val mask = new Keypad.State
  }
}