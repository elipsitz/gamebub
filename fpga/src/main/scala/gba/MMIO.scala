package gba

import chisel3._
import chisel3.util._
import gba.mem.TargetInterface
import lib.log.Logger

class MmioTarget extends Bundle {
  /// *Word* address
  val address = Input(UInt(9.W))
  val request = Input(Bool())
  val write = Input(Bool())
  val mask = Input(UInt(4.W))
  val dataWrite = Input(UInt(32.W))
  val dataRead = Output(UInt(32.W))
  /// Whether the access is to a valid register
  val valid = Output(Bool())
}

/// GBA MMIO bus
///
/// All registers are 32-bits (individual targets adjust this as needed based on mask).
/// All accesses are asynchronous / same-cycle
class MMIO(numTargets: Int) extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val mem = new TargetInterface(32.W)
  })
  private val logger = Logger("mmio", enable = io.enable)
  val targets: Seq[MmioTarget] = Seq.fill(numTargets)(IO(Flipped(new MmioTarget)))

  val queuedRequest = RegInit(false.B)
  val queuedWrite = RegInit(false.B)
  val queuedAddress = Reg(UInt(14.W))
  val queuedMask = Reg(UInt(4.W))
  val queuedAddressValid = RegInit(false.B)

  val isValid = VecInit(targets.map(_.valid)).asUInt.orR
  for (target <- targets) {
    target.address := queuedAddress
    target.request := queuedRequest && queuedAddressValid && io.enable
    target.write := queuedWrite
    target.mask := queuedMask
    target.dataWrite := io.mem.dataWrite
  }

  when (isValid && queuedAddressValid) {
    io.mem.dataRead := Mux1H(targets.map(t => (t.valid, t.dataRead)))
  } .elsewhen (queuedAddress === (0x800 >> 2).U) {
    // Stubbed out special "Internal Memory Control" register
    io.mem.dataRead := "h0D000020".U
  } .otherwise {
    // TODO: handle open bus
    io.mem.dataRead := 0.U
  }

  io.mem.done := false.B
  when (io.enable) {
    when (queuedRequest) {
      when (queuedWrite) {
        logger.debug(cf"write addr=0x${queuedAddress * 4.U}%x data=${io.mem.dataWrite}%x mask=${queuedMask}%b")
      } .otherwise {
        logger.debug(cf"read  addr=0x${queuedAddress * 4.U}%x data=${io.mem.dataRead}%x")
      }

      queuedRequest := false.B
      io.mem.done := true.B
    }
    when (io.mem.request) {
      queuedRequest := true.B
      queuedWrite := io.mem.write
      queuedAddress := io.mem.address >> 2
      queuedAddressValid := io.mem.address < 0x800.U
      queuedMask := io.mem.mask
    }
  }
}

object MMIO {
  def mask[T <: Data](oldData: T, newData: Data, byteMask: UInt): T = {
    val width = oldData.getWidth.max(newData.getWidth)
    val numBytes = (width + 8 - 1) / 8
    assert(byteMask.getWidth == numBytes, "mask is wrong width")

    val newPad = newData.asUInt.pad(numBytes * 8)
    val oldPad = oldData.asUInt.pad(numBytes * 8)
    val newVec = VecInit((0 until numBytes).map(i => newPad(i * 8 + 7, i * 8)))
    val oldVec = VecInit((0 until numBytes).map(i => oldPad(i * 8 + 7, i * 8)))
    val combined = VecInit((0 until numBytes).map(i => Mux(byteMask(i), newVec(i), oldVec(i))))
    combined.asTypeOf(oldData)
  }
}

object MmioMap {
  def apply (entries: (Int, Entry)*): MmioTarget = fromSeq(entries)

  def fromSeq(entries: Seq[(Int, Entry)]): MmioTarget = {
    // Ensure addresses are word-aligned and in bounds.
    for ((addr, i) <- entries.map(_._1).zipWithIndex) {
      if (addr % 4 != 0) {
        throw new IllegalArgumentException(f"entry $i (at 0x$addr%x) is not aligned")
      }
      if (addr >= (1 << 11)) {
        throw new IllegalArgumentException(f"entry $i (at 0x$addr%x) is larger than address width")
      }
    }

    val interface = Wire(new MmioTarget)
    interface.dataRead := DontCare
    interface.valid := false.B

    entries.foreach { case (address, reg) =>
      when (interface.request && interface.address === (address / 4).U) {
        val (readData, readValid) = reg.read.fn(!interface.write)
        reg.write.fn(interface.write, interface.dataWrite, interface.mask)
        interface.valid := readValid

        when (!interface.write) {
          interface.dataRead := readData
        }
      }
    }

    interface
  }

  // (enable) => (data, valid)
  case class ReadFn(fn: Bool => (UInt, Bool))
  object ReadFn {
    // Simple read from a register or wire.
    def apply(reg: Data): ReadFn = {
      assert(reg.getWidth <= 32)
      ReadFn(_ => (reg.asUInt, true.B))
    }

    // Read from two 16-bit registers.
    def apply(reg0: Data, reg1: Data): ReadFn = {
      assert(reg0.getWidth <= 16)
      assert(reg1.getWidth <= 16)
      val data = Cat(reg1.asUInt.pad(16), reg0.asUInt.pad(16))
      ReadFn(_ => (data, true.B))
    }

    // Read from four 8-bit registers.
    def apply(reg0: Data, reg1: Data, reg2: Data, reg3: Data): ReadFn = {
      assert(reg0.getWidth <= 8)
      assert(reg1.getWidth <= 8)
      assert(reg2.getWidth <= 8)
      assert(reg3.getWidth <= 8)
      val data = Cat(reg3.asUInt.pad(8), reg2.asUInt.pad(8), reg1.asUInt.pad(8), reg0.asUInt.pad(8))
      ReadFn(_ => (data, true.B))
    }

    // No-op read.
    def apply(): ReadFn = ReadFn(_ => (0.U, false.B))
  }

  // (enable, data, mask) => ()
  case class WriteFn(fn: (Bool, UInt, UInt) => Unit)
  object WriteFn {
    // Simple write to a register.
    def apply(reg: Data): WriteFn = WriteFn((enable, data, mask) => {
      when (enable) {
        reg := MMIO.mask(reg, data, mask)
      }
    })

    // Write to two 16-bit registers.
    def apply(reg0: Data, reg1: Data): WriteFn = WriteFn((enable, data, mask) => {
      when (enable) {
        reg0 := MMIO.mask(reg0, data(15, 0), mask(1, 0))
        reg1 := MMIO.mask(reg1, data(31, 16), mask(3, 2))
      }
    })

    // Write to four 8-bit registers.
    def apply(reg0: Data, reg1: Data, reg2: Data, reg3: Data): WriteFn = WriteFn((enable, data, mask) => {
      when (enable) {
        reg0 := MMIO.mask(reg0, data(7, 0), mask(0))
        reg1 := MMIO.mask(reg1, data(15, 8), mask(1))
        reg2 := MMIO.mask(reg2, data(23, 16), mask(2))
        reg3 := MMIO.mask(reg3, data(31, 24), mask(3))
      }
    })

    // No-op write.
    def apply(): WriteFn = WriteFn((_, _, _) => ())
  }

  case class Entry(read: ReadFn, write: WriteFn)
  object Entry {
    def r(reg: Data): Entry = Entry(ReadFn(reg), WriteFn())

    def w(reg: Data): Entry = Entry(ReadFn(), WriteFn(reg))

    def rw(reg: Data): Entry = Entry(ReadFn(reg), WriteFn(reg))

    def w16(reg0: Data, reg1: Data): Entry = Entry(ReadFn(), WriteFn(reg0, reg1))

    def rw16(reg0: Data, reg1: Data): Entry = Entry(ReadFn(reg0, reg1), WriteFn(reg0, reg1))

    def w8(reg0: Data, reg1: Data, reg2: Data, reg3: Data): Entry
      = Entry(ReadFn(), WriteFn(reg0, reg1, reg2, reg3))

    def rw8(reg0: Data, reg1: Data, reg2: Data, reg3: Data): Entry
      = Entry(ReadFn(reg0, reg1, reg2, reg3), WriteFn(reg0, reg1, reg2, reg3))
  }
}
