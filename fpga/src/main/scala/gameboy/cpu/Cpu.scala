package gameboy.cpu

import chisel3._
import chisel3.util._
import gameboy.{Clocker, Gameboy}
import gameboy.cpu.Cpu.DebugState

object Cpu {
  class InterruptFlags extends Bundle {
    val joypad = Bool()
    val serial = Bool()
    val timer = Bool()
    val lcdStat = Bool()
    val vblank = Bool()
  }

  val REG_IE = 0xFFFF;
  val REG_IF = 0xFF0F;

  class DebugState extends Bundle {
    val regB = UInt(8.W)
    val regC = UInt(8.W)
    val regD = UInt(8.W)
    val regE = UInt(8.W)
    val regH = UInt(8.W)
    val regL = UInt(8.W)
    val regF = UInt(8.W)
    val regA = UInt(8.W)
    val regSp = UInt(16.W)
    val regPc = UInt(16.W)
  }
}

/** Gameboy CPU - Sharp SM83 */
class Cpu(config: Gameboy.Configuration) extends Module {
  val io = IO(new Bundle {
    val clocker = Input(new Clocker)

    /** System bus address selection */
    val memAddress = Output(UInt(16.W))
    /** System bus access enable */
    val memEnable = Output(Bool())
    /** System bus write enable */
    val memWrite = Output(Bool())
    /** System bus data in */
    val memDataIn = Input(UInt(8.W))
    /** System bus data out */
    val memDataOut = Output(UInt(8.W))
    /** Interrupt requests from peripherals */
    val interruptRequest = Input(new Cpu.InterruptFlags)
    /** Whether the CPU is in the STOP state */
    val stopState = Output(Bool())
    /** Whether the CPU should exit the STOP state */
    val stopStateExit = Input(Bool())

    /** Debug */
    val debugState = Output(new DebugState)
  })

  val control = Module(new Control(config))
  io.stopState := control.io.stopState
  control.io.stopStateExit := io.stopStateExit
  val controlSignals = control.io.signals
  val alu = Module(new Alu())
  val aluFlagNext = Wire(new Alu.Flags)
  val memDataRead = WireDefault(io.memDataIn)

  // Interrupts
  // IE stores all 8 bits (only the low 5 are used); IF's top 3 bits always read as 1.
  val regIE = RegInit(0.U(8.W))
  val regIF = RegInit(0.U(5.W))
  when (io.clocker.enable) {
    regIF := regIF | io.interruptRequest.asUInt
    when (io.memEnable) {
      when (io.memWrite) {
        when (io.memAddress === Cpu.REG_IE.U) { regIE := io.memDataOut }
        when (io.memAddress === Cpu.REG_IF.U) { regIF := io.memDataOut(4, 0) | io.interruptRequest.asUInt }
      }.otherwise {
        when (io.memAddress === Cpu.REG_IE.U) { memDataRead := regIE }
        when (io.memAddress === Cpu.REG_IF.U) { memDataRead := Cat("b111".U(3.W), regIF) }
      }
    }
  }
  val pendingInterruptField = regIE(4, 0) & regIF
  val pendingInterruptIndex = PriorityEncoder(pendingInterruptField)
  control.io.interruptsPending := pendingInterruptField =/= 0.U
  when (io.clocker.phiPulse && controlSignals.pcNext === PcNext.interrupt) {
    // Final step of interrupt process: jump to interrupt (and ack it in 'IF').
    regIF := regIF & (~(1.U(1.W) << pendingInterruptIndex)).asUInt
  }

  // Control
  control.io.clocker := io.clocker
  control.io.memDataIn := memDataRead
  val instructionRegister = RegEnable(memDataRead, 0.U(8.W), io.clocker.phiPulse && controlSignals.instLoad)

  // Registers
  // Includes incrementer/decrementer.
  // 14 Registers: BC DE HL FA SP WZ PC
  //               01 23 45 67 89 AB CD
  // TODO: doesn't work anymore with skipBootrom
  val initialRegisterValues = Seq.fill(14)(0x00)
  val registers = RegInit(VecInit(initialRegisterValues.map(_.U(8.W))))
  val regR16IndexHi = Wire(UInt(4.W))
  regR16IndexHi := 0.U
  val regR16IndexLo = regR16IndexHi | 1.U
  switch (instructionRegister(5, 4)) {
    is("b00".U) { regR16IndexHi := 0.U; }
    is("b01".U) { regR16IndexHi := 2.U; }
    is("b10".U) { regR16IndexHi := 4.U; }
    is("b11".U) { regR16IndexHi := 8.U; }
  }
  private def genRegisterIndex(selector: RegSel.Type) = {
    val index = WireDefault(0.U(4.W))
    switch (selector) {
      is(RegSel.a) { index := 7.U }
      is(RegSel.f) { index := 6.U }
      is(RegSel.c) { index := 1.U }
      is(RegSel.w) { index := 10.U }
      is(RegSel.z) { index := 11.U }
      is(RegSel.h) { index := 4.U }
      is(RegSel.l) { index := 5.U }
      is(RegSel.reg8Src) { index := Cat(0.U(1.W), instructionRegister(2, 0)) }
      is(RegSel.reg8Dest) { index := Cat(0.U(1.W), instructionRegister(5, 3)) }
      is(RegSel.reg16Hi) { index := regR16IndexHi }
      is(RegSel.reg16Lo) { index := regR16IndexLo }
      is(RegSel.spHi) { index := 8.U }
      is(RegSel.spLo) { index := 9.U }
      is(RegSel.pcHi) { index := 12.U }
      is(RegSel.pcLo) { index := 13.U }
    }
    index
  }
  val regRead1Index = genRegisterIndex(controlSignals.regRead1Sel)
  val regRead2Index = genRegisterIndex(controlSignals.regRead2Sel)
  val regWriteIndex = genRegisterIndex(controlSignals.regWriteSel)
  val flagRead = registers(6)(7, 4).asTypeOf(new Alu.Flags)
  val pc = Cat(registers(12), registers(13))
  val regRead1Out = registers(regRead1Index)
  val regRead2Out = registers(regRead2Index)
  // Incrementer
  val incIn = WireDefault(0.U(16.W))
  val incOut = WireDefault(0.U(16.W))
  switch (controlSignals.incReg) {
    is (IncReg.pc) { incIn := pc }
    is (IncReg.hl) { incIn := Cat(registers(4), registers(5)) }
    is (IncReg.sp) { incIn := Cat(registers(8), registers(9)) }
    is (IncReg.wz) { incIn := Cat(registers(10), registers(11)) }
    is (IncReg.inst16) { incIn := Cat(registers(regR16IndexHi), registers(regR16IndexLo)) }
    is (IncReg.pcAlu) { incIn := Cat(alu.io.out, registers(13)) }
  }
  switch (controlSignals.incOp) {
    is (IncOp.none) { incOut := incIn }
    is (IncOp.inc, IncOp.incNoWrite) { incOut := incIn + 1.U }
    is (IncOp.dec) { incOut := incIn - 1.U }
  }
  // Register write
  when (io.clocker.phiPulse) {
    switch (controlSignals.regOp) {
      is (RegOp.writeAlu) { registers(regWriteIndex) := alu.io.out }
      is (RegOp.writeMem) {
        // Keep bottom 4 bits of Flags register 0
        registers(regWriteIndex) := Mux(
          regWriteIndex === 6.U,
          Cat(memDataRead(7, 4), 0.U(4.W)),
          memDataRead
        )
      }
    }
    when (controlSignals.incOp === IncOp.inc | controlSignals.incOp === IncOp.dec) {
      switch (controlSignals.incReg) {
        is (IncReg.hl) {
          registers(4) := incOut(15, 8)
          registers(5) := incOut(7, 0)
        }
        is (IncReg.sp) {
          registers(8) := incOut(15, 8)
          registers(9) := incOut(7, 0)
        }
        is (IncReg.wz) {
          registers(10) := incOut(15, 8)
          registers(11) := incOut(7, 0)
        }
        is (IncReg.inst16) {
          registers(regR16IndexHi) := incOut(15, 8)
          registers(regR16IndexLo) := incOut(7, 0)
        }
      }
    }
    switch (controlSignals.pcNext) {
      is (PcNext.incOut) {
        registers(12) := incOut(15, 8)
        registers(13) := incOut(7, 0)
      }
      is (PcNext.rstAddr) {
        registers(12) := 0.U
        registers(13) := Cat(0.U(2.W), instructionRegister(5, 3), 0.U(3.W))
      }
      is (PcNext.interrupt) {
        registers(12) := 0.U
        registers(13) := Cat("b01".U(2.W), pendingInterruptIndex, 0.U(3.W))
      }
    }
    when (controlSignals.aluFlagSet =/= AluFlagSet.setNone) {
      registers(6) := Cat(aluFlagNext.asUInt, 0.U(4.W))
    }
  }

  // ALU
  val aluCarry = RegEnable(alu.io.flagOut.c, 0.B, io.clocker.phiPulse & controlSignals.aluOp === AluOp.addLo)
  alu.io.a := DontCare
  switch (controlSignals.aluSelA) {
    is (AluSelA.regA) { alu.io.a := registers(7) }
    is (AluSelA.reg1) { alu.io.a := regRead1Out }
  }
  alu.io.b := DontCare
  switch (controlSignals.aluSelB) {
    is (AluSelB.reg2) { alu.io.b := regRead2Out }
    is (AluSelB.signReg2) { alu.io.b := Fill(8, regRead2Out(7)) }
  }
  alu.io.op := DontCare
  alu.io.flagIn := flagRead
  switch (controlSignals.aluOp) {
    is (AluOp.copyA) { alu.io.op := Alu.Opcode.copyA }
    is (AluOp.copyB) { alu.io.op := Alu.Opcode.copyB }
    is (AluOp.incB) { alu.io.op := Alu.Opcode.incB }
    is (AluOp.decB) { alu.io.op := Alu.Opcode.decB }
    is (AluOp.instAlu) { alu.io.op := Cat("b00".U(2.W), instructionRegister(5, 3)).asTypeOf(Alu.Opcode()) }
    is (AluOp.instAcc) { alu.io.op := Cat("b01".U(2.W), instructionRegister(5, 3)).asTypeOf(Alu.Opcode()) }
    is (AluOp.instCB) { alu.io.op := Cat("b10".U(2.W), instructionRegister(5, 3)).asTypeOf(Alu.Opcode()) }
    is (AluOp.instBit) { alu.io.op := Cat("b111".U(3.W), instructionRegister(7, 6)).asTypeOf(Alu.Opcode()) }
    is(AluOp.addLo) { alu.io.op := Alu.Opcode.add }
    is(AluOp.addHi) {
      alu.io.op := Alu.Opcode.adc
      alu.io.flagIn.c := aluCarry
    }
  }
  alu.io.bitIndex := instructionRegister(5, 3)
  aluFlagNext := alu.io.flagOut
  switch (controlSignals.aluFlagSet) {
    is (AluFlagSet.setNone) { aluFlagNext := alu.io.flagIn }
    is (AluFlagSet.setAll) { aluFlagNext := alu.io.flagOut }
    is (AluFlagSet.set_NHC) {
      aluFlagNext.z := alu.io.flagIn.z
    }
    is(AluFlagSet.set0NHC) {
      aluFlagNext.z := 0.U
    }
  }

  // Condition code checking
  control.io.condition := false.B
  switch (instructionRegister(4, 3)) {
    is (0.U) { control.io.condition := !flagRead.z}
    is (1.U) { control.io.condition := flagRead.z}
    is (2.U) { control.io.condition := !flagRead.c}
    is (3.U) { control.io.condition := flagRead.c}
  }

  // Memory accesses
  io.memDataOut := alu.io.out
  io.memAddress := DontCare
  switch (controlSignals.memAddrSel) {
    is (MemAddrSel.incrementer) { io.memAddress := incIn }
    is (MemAddrSel.high) { io.memAddress := Cat(0xFF.U(8.W), regRead2Out) }
  }
  io.memEnable := controlSignals.memEnable
  io.memWrite := controlSignals.memWrite

  // Debug output
  io.debugState.regB := registers(0)
  io.debugState.regC := registers(1)
  io.debugState.regD := registers(2)
  io.debugState.regE := registers(3)
  io.debugState.regH := registers(4)
  io.debugState.regL := registers(5)
  io.debugState.regF := registers(6)
  io.debugState.regA := registers(7)
  io.debugState.regSp := Cat(registers(8), registers(9))
  io.debugState.regPc := Cat(registers(12), registers(13))
//  when (io.clocker.phiPulse) {
//    printf(cf"* PC=$pc%x inst=${instructionRegister}%x B=${registers(0)}%x C=${registers(1)}%x D=${registers(2)}%x E=${registers(3)}%x HL=${Cat(registers(4), registers(5))}%x F=${registers(6)}%x A=${registers(7)}%x SP=${Cat(registers(8),registers(9))}%x\n")
//  }
}
