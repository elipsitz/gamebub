package gba

import chisel3._
import chisel3.util._
import lib.log.Logger
import chisel3.{BitsIntf => ack}

class Interrupt extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val mmio = new MmioTarget()

    /// IRQ signal to the CPU
    val irq = Output(Bool())

    /// IRQ signals from peripherals
    val peripheralIrq = Input(new Interrupt.Flags)

    /// Whether the CPU is halted
    val cpuHalt = Output(Bool())
    /// Whether the BIOS is unlocked
    val biosUnlocked = Input(Bool())
  })
  val logger = Logger("interrupt", enable = io.enable)

  val ime = RegInit(0.U(1.W))
  val regEnabled = RegInit(0.U.asTypeOf(new Interrupt.Flags))
  val regRequested = RegInit(0.U.asTypeOf(new Interrupt.Flags))
  val regCpuHalt = RegInit(false.B)
  val reg410 = RegInit(0.U(32.W))

  val irqActive = (regRequested.asUInt & regEnabled.asUInt) =/= 0.U
  val irqAck = WireDefault(0.U((new Interrupt.Flags).getWidth.W))

  // 3 cycle delay on IRQ input signal, likely synchronizers
  val irqDelay = ShiftRegister(ime(0) && irqActive, 3, io.enable)
  val haltExitDelay = ShiftRegister(irqActive, 3, io.enable)

  when (io.enable) {
    regRequested := ((regRequested.asUInt & (~irqAck)) | io.peripheralIrq.asUInt).asTypeOf(new Interrupt.Flags)

    when (haltExitDelay) {
      // Exiting HALT happens when IE & IF, regardless of IME (or CPU irq)
      regCpuHalt := false.B
      logger.info(cf"CPU resumed")
    }
  }
  io.irq := irqDelay
  io.cpuHalt := regCpuHalt

  io.mmio <> MmioMap(
    // 0x200: IE, 0x202: IF
    0x200 -> MmioMap.Entry(
      MmioMap.ReadFn(_ => {
        val data = Cat(
          regRequested.asUInt.pad(16),
          regEnabled.asUInt.pad(16),
        )
        (data, true.B)
      }),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          // Write to IE
          regEnabled := MMIO.mask(regEnabled, data(15, 0), mask(1, 0))
          // Write to IF
          irqAck := MMIO.mask(0.U(16.W), data(31, 16), mask(3, 2))
        }
      })
    ),
    // 0x208: IME
    0x208 -> MmioMap.Entry.rw(ime),
    // 0x300: POSTFLG and HALTCNT
    0x300 -> MmioMap.Entry(
      MmioMap.ReadFn(_ => (0.U, true.B)),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable && io.biosUnlocked && mask(1)) {
          val haltmode = data(15)
          when (haltmode === 0.U) {
            logger.info(cf"CPU halted")
            regCpuHalt := true.B
          } .otherwise {
            logger.warn(cf"HALTCNT = 1 (STOP) not implemented")
          }
        }
      })
    ),
    // 0x410: Undocumented BIOS
    0x410 -> MmioMap.Entry(
      MmioMap.ReadFn(_ => ((~reg410).asUInt ^ "h0A0B8FEE".U(32.W), true.B)),
      MmioMap.WriteFn(reg410),
    )
  )
}

object Interrupt {
  class Flags extends Bundle {
    val cartridge = Bool()
    val keypad = Bool()
    val dma = UInt(4.W)
    val link = Bool()
    val timer = UInt(4.W)
    val vcount = Bool()
    val hblank = Bool()
    val vblank = Bool()
  }
}