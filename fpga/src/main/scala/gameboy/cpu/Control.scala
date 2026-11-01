package gameboy.cpu

import chisel3._
import chisel3.experimental.BundleLiterals.AddBundleLiteralConstructor
import chisel3.util._
import gameboy.{Clocker, Gameboy}
import gameboy.util.MemRomTable

import scala.io.Source

object Microbranch extends ChiselEnum {
  val next, jump, cond, dispatch = Value
}
object DispatchPrefix extends ChiselEnum {
  val none = Value(0.U)
  val cb = Value(1.U)
}
object ImeUpdate extends ChiselEnum {
  val same, enable, disable = Value
}

/** Control signal: how PC should be updated on an M-cycle. */
object PcNext extends ChiselEnum {
  /** Do not change PC. */
  val same = Value
  /** PC = output of incrementer/decrementer */
  val incOut = Value
  /** PC = computed reset address */
  val rstAddr = Value
  /** PC = computed interrupt address */
  val interrupt = Value
}

/** Control signal: 8-bit register select. */
object RegSel extends ChiselEnum {
  /** The accumulator register. */
  val a = Value
  /** The flags register. */
  val f = Value
  /** Register C. */
  val c = Value
  /** Temp register W. */
  val w = Value
  /** Temp register Z. */
  val z = Value
  /** Register H. */
  val h = Value
  /** Register L. */
  val l = Value
  /** The 8-bit register denoted by bits 2:0. */
  val reg8Src = Value
  /** The 8-bit register denoted by bits 5:3. */
  val reg8Dest = Value
  /** The high part of the 16-bit register denoted by bits 5:4. */
  val reg16Hi = Value
  /** The low part of the 16-bit register denoted by bits 5:4. */
  val reg16Lo = Value
  /** High byte of SP register. */
  val spHi = Value
  /** Low byte of SP register. */
  val spLo = Value
  /** High byte of PC register. */
  val pcHi = Value
  /** Low byte of PC register. */
  val pcLo = Value
}

/** Control signal: register write operation. */
object RegOp extends ChiselEnum {
  /** Do not write a register. */
  val none = Value
  /** Write the output of the ALU. */
  val writeAlu = Value
  /** Write the memory data input. */
  val writeMem = Value
}

/** Control signal: register incrementer operation. */
object IncOp extends ChiselEnum {
  /** Do nothing. */
  val none = Value
  /** Increment. */
  val inc = Value
  /** Decrement. */
  val dec = Value
  /** Increment but no write-back. */
  val incNoWrite = Value
}

/** Control signal: register incrementer target selector. */
object IncReg extends ChiselEnum {
  /** PC register */
  val pc = Value
  /** HL register */
  val hl = Value
  /** SP register */
  val sp = Value
  /** WZ register */
  val wz = Value
  /** 16-bit register selected by instruction register */
  val inst16 = Value
  /** Target: PC register, Input: {ALU Out, PC Lo} (used for relative jump). */
  val pcAlu = Value
}

/** Control signal: ALU operation. */
object AluOp extends ChiselEnum {
  /** Output = A */
  val copyA = Value
  /** Output = B */
  val copyB = Value
  /** Output = B + 1 */
  val incB = Value
  /** Output = B - 1 */
  val decB = Value
  /** Output = A + B (and set the internal carry flag) */
  val addLo = Value
  /** Output = A + B (and use the internal carry flag) */
  val addHi = Value
  /** Use the "ALU opcode" from the instruction (ADD/ADC/SUB/SBC/AND/XOR/OR/CP). */
  val instAlu = Value
  /** Use the "Acc/Flag opcode" from the instruction (RLCA/RRCA/RLA/RRA/DAA/CPL/SCF/CCF). */
  val instAcc = Value
  /** Use the CB-prefixed opcode from the instruction. */
  val instCB = Value
  /** CB-prefixed single bit operations (get/reset/set) */
  val instBit = Value
}

/** Control signal: ALU operand A source. */
object AluSelA extends ChiselEnum {
  /** A = Accumulator register */
  val regA = Value
  /** A = Register Read 1, */
  val reg1 = Value
}

/** Control signal: ALU operand B source. */
object AluSelB extends ChiselEnum {
  /** B = Register Read 2 */
  val reg2 = Value
  /** B = Sign extension of Register Read 2 */
  val signReg2 = Value
}

/** Control signal: ALU flag set mode. */
object AluFlagSet extends ChiselEnum {
  /** F = ---- (no change) */
  val setNone = Value
  /** F = **** (all set) */
  val setAll = Value
  /** F = -*** (all set except zero) */
  val set_NHC = Value
  /** F = 0*** (all set, zero unset) */
  val set0NHC = Value
}

/** Control signal: where the memory load/store address comes from. */
object MemAddrSel extends ChiselEnum {
  /** **INPUT** to the register incrementer (not output). */
  val incrementer = Value
  /** High address: 0xFF00 | (Register 2) */
  val high = Value
}

class ControlSignals extends Bundle {
  /** how PC should be updated */
  val pcNext = PcNext()
  /** if the instruction reg. should be loaded with memory read */
  val instLoad = Bool()
  /** the first register we're reading */
  val regRead1Sel = RegSel()
  /** the second register we're reading */
  val regRead2Sel = RegSel()
  /** the register we're (maybe) writing to */
  val regWriteSel = RegSel()
  /** register write operation to perform */
  val regOp = RegOp()
  /** incrementer/decrementer operation to perform */
  val incOp = IncOp()
  /** incrementer/decrementer register target */
  val incReg = IncReg()
  /** ALU operation */
  val aluOp = AluOp()
  /** ALU select A */
  val aluSelA = AluSelA()
  /** ALU select B */
  val aluSelB = AluSelB()
  /** whether ALU should update flags */
  val aluFlagSet = AluFlagSet()
  /** whether we're accessing memory */
  val memEnable = Bool()
  /** whether we're writing to memory (if `mem_enable`). */
  val memWrite = Bool()
  /** where the memory address comes from */
  val memAddrSel = MemAddrSel()
}

class Control(config: Gameboy.Configuration) extends Module {
  val io = IO(new Bundle {
    val clocker = Input(new Clocker)

    /** Current memory data in. */
    val memDataIn = Input(UInt(8.W))
    /** Whether the current condition (in instruction register) is satisfied based on flags. */
    val condition = Input(Bool())
    /** Whether interrupts are pending (IE & IF) =/= 0 */
    val interruptsPending = Input(Bool())
    /** Whether CPU is in the 'STOP' state */
    val stopState = Output(Bool())
    /** Whether the CPU should exit the 'STOP' state */
    val stopStateExit = Input(Bool())

    /** Control signals */
    val signals = Output(new ControlSignals)
  })

  val microcode = new Microcode(Source.fromResource("microcode.csv"))

  class MicrocodeRow extends Bundle {
    val microOp = Microbranch()
    val nextState = UInt(microcode.stateWidth())
    val dispatchPrefix = DispatchPrefix()
    val imeUpdate = ImeUpdate()
    val signals = new ControlSignals
  }

  val state = RegInit(0.U(microcode.stateWidth()))
  val ime = RegInit(false.B)

  val controlTable = Module(new MemRomTable(
    config,
    new MicrocodeRow,
    microcode.entries.map(e => {
      val row = (new MicrocodeRow).Lit(
        _.microOp -> e.microOp,
        _.nextState -> e.nextState().getOrElse(0).U,
        _.dispatchPrefix -> e.dispatchPrefix.getOrElse(DispatchPrefix.none),
        _.imeUpdate -> e.imeUpdate.getOrElse(ImeUpdate.same),
        _.signals -> (new ControlSignals).Lit(
          _.pcNext -> e.pcNext.getOrElse(PcNext.same),
          _.instLoad -> e.instLoad.getOrElse(false).B,
          _.regRead1Sel -> e.regRead1Sel.getOrElse(RegSel.a),
          _.regRead2Sel -> e.regRead2Sel.getOrElse(RegSel.a),
          _.regWriteSel -> e.regWriteSel.getOrElse(RegSel.a),
          _.regOp -> e.regOp.getOrElse(RegOp.none),
          _.incOp -> e.incOp.getOrElse(IncOp.none),
          _.incReg -> e.incReg.getOrElse(IncReg.pc),
          _.aluOp -> e.aluOp.getOrElse(AluOp.copyA),
          _.aluSelA -> e.aluSelA.getOrElse(AluSelA.regA),
          _.aluSelB -> e.aluSelB.getOrElse(AluSelB.reg2),
          _.aluFlagSet -> e.aluFlagSet.getOrElse(AluFlagSet.setNone),
          _.memEnable -> e.memEnable.getOrElse(false).B,
          _.memWrite -> e.memWrite.getOrElse(false).B,
          _.memAddrSel -> e.memAddrSel.getOrElse(MemAddrSel.incrementer),
        )
      )
      row
    })
  ))
  controlTable.io.addr := state
  val row = controlTable.io.data
  io.signals := row.signals

  val halted = state === microcode.stateForLabel("HALT").U
  val justFetched = RegInit(false.B)
  when (io.clocker.enable) {
    justFetched := false.B
  }

  when (io.clocker.enable && io.clocker.tCycle === 0.U && io.interruptsPending && (justFetched || halted)) {
    // Handle interrupts.
    when (ime && halted) {
      // If IME and halted is set, start the interrupt routine without decrementing the PC,
      // so that upon RETI, we continue after the HALT instruction.
      state := microcode.stateForLabel("#Interrupt_keepPC").U
    } .elsewhen (ime) {
      // Note: IME is unset in microcode
      state := microcode.stateForLabel("#Interrupt").U
    } .elsewhen (halted && justFetched) {
      // HALT bug: HALT was executed with IME=0 and an interrupt already pending,
      // so the CPU doesn't halt, and the next byte is read twice (PC isn't incremented).
      state := microcode.stateForLabel("#HALT_bug").U
    } .elsewhen (halted) {
      // Go to the next instruction without doing an interrupt.
      state := microcode.stateForLabel("NOP").U
    }
  }

  when (io.clocker.phiPulse) {
    // Advance the state machine.
    switch (row.microOp) {
      is (Microbranch.next) {
        state := state + 1.U
      }
      is (Microbranch.dispatch) {
        val dispatchTable = Module(new MemRomTable(
          config,
          UInt(microcode.stateWidth()),
          (0 until 512).map(i =>
            microcode.entries.zipWithIndex
              .flatMap(e => e._1.encoding.map((_, e._2)))
              .find(e => e._1.value == (i & e._1.mask))
              .map(e => e._2.U)
              .getOrElse(microcode.stateForLabel("_INVALID").U)
          )
        ))
        val dispatchKey = Cat(row.dispatchPrefix.asUInt, io.memDataIn)
        dispatchTable.io.addr := dispatchKey
        state := dispatchTable.io.data
        justFetched := row.dispatchPrefix === DispatchPrefix.none
      }
      is (Microbranch.jump) {
        state := row.nextState
      }
      is (Microbranch.cond) {
        state := Mux(io.condition, row.nextState, state + 1.U)
      }
    }

    // Maybe enable/disable IME.
    val delayedImeSet = RegInit(false.B)
    when (delayedImeSet) {
      ime := true.B
      delayedImeSet := false.B
    }
    when (row.imeUpdate === ImeUpdate.enable) { delayedImeSet := true.B }
    when (row.imeUpdate === ImeUpdate.disable) { ime := false.B}
  }

  // Handle STOP mode.
  // NOTE: 'STOP' actually has much more complex and inconsistent behavior...
  // but for this we're only doing what's needed for speed switching
  val stopped = state === microcode.stateForLabel("#STOP_loop").U
  io.stopState := stopped
  when (io.clocker.enable && stopped && io.stopStateExit) {
    // Set this after microcode table above so it takes effect.
    state := microcode.stateForLabel("NOP").U
  }
}