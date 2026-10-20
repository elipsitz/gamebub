package gameboy

import chisel3._
import chisel3.util._

class Joypad extends Module {
  val io = IO(new PeripheralAccess {
    val clocker = Input(new Clocker)
    val state = Input(new JoypadState())
    val interruptRequest = Output(Bool())
  })

  val selectDirection = RegInit(false.B)
  val selectAction = RegInit(false.B)
  val readState = Cat(
    !((io.state.down && selectDirection) || (io.state.start && selectAction)),
    !((io.state.up && selectDirection) || (io.state.select && selectAction)),
    !((io.state.left && selectDirection) || (io.state.b && selectAction)),
    !((io.state.right && selectDirection) || (io.state.a && selectAction)),
  )

  io.dataRead := Cat("b11".U(2.W), !selectAction, !selectDirection, readState)
  // IRQ when any bit in readState goes from high to low
  io.interruptRequest := (RegEnable(readState, 0.U, io.clocker.enable) & (~readState).asUInt).orR

  io.valid := false.B
  when (io.enabled && io.address === 0x00.U) {
    io.valid := true.B
    when (io.write) {
      selectDirection := !io.dataWrite(4)
      selectAction := !io.dataWrite(5)
    }
  }
}