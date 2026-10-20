package gameboy.cart

import chisel3._
import chisel3.util._
import gameboy.Clocker
import gameboy.cart.CartridgeController.BusRequester

object CartridgeController {
  object BusRequester extends ChiselEnum {
    /// CPU is controlling the bus
    val cpu = Value
    /// OAM DMA is controlling the bus
    val oamDma = Value
    /// CGB VRAM DMA is controlling the bus
    val vramDma = Value
  }
}

class CartridgeController extends Module {
  val io = IO(new Bundle {
    val clocker = Input(new Clocker)
    // Whether we're on the last (system clock) cycle of an M-clock cycle.
    val lastClockCycle = Input(Bool())

    /// Cartridge interface
    val cartridge = new CartridgeInterface

    // Bus interface
    val busRequester = Input(CartridgeController.BusRequester())
    val busEnable = Input(Bool())
    val busAddress = Input(UInt(16.W))
    val busWrite = Input(Bool())
    // 1 for ROM, 0 for RAM
    val busIsRom = Input(Bool())
    val busDataRead = Output(UInt(8.W))
    val busDataWrite = Input(UInt(8.W))
  })

  // Physical interface
  val regAddressOut = RegInit(0.U(16.W))
  val regDataOut = RegInit(0.U(8.W))
  val reg_nRD = RegInit(false.B)
  val reg_nWR = RegInit(true.B)
  val reg_nCS = RegInit(true.B)
  val reg_phi = RegInit(true.B)

  io.cartridge.phi := reg_phi
  io.cartridge.nWR := reg_nWR
  io.cartridge.nRD := reg_nRD
  io.cartridge.nCS := reg_nCS
  io.cartridge.address := regAddressOut
  io.cartridge.dataOut := regDataOut
  io.busDataRead := io.cartridge.dataIn
  io.cartridge.dataDir := ~reg_nWR // Output if writing

  // TODO: some signals actually start immediately in tCycle=0,
  // which means that the memory access needs to be available at the very end of a phi
  // cycle, rather than at the beginning of the next one.
  when (io.clocker.enable) {
    switch (io.clocker.tCycle) {
      is (0.U) {
        when (io.busEnable) {
          regAddressOut := io.busAddress
          regDataOut := io.busDataWrite

          reg_nRD := io.busWrite
          reg_nWR := ~io.busWrite
          reg_nCS := io.busIsRom
        } .otherwise {
          // idle, or 0xFE00-0xFFFF
          regAddressOut := 0xFFFF.U(16.W)
          reg_nRD := 0.U
          reg_nWR := 1.U
          reg_nCS := 1.U
        }
      }
      is (1.U) {
        // TODO: phi should be stopped when in STOP (~15ms during a speed transition)
        reg_phi := 0.U
      }
      is (2.U) {
        reg_nWR := 1.U  // nWR only stays low for 2 cycles (well, 1.5)
      }
      is (3.U) {
        reg_phi := 1.U
        reg_nCS := 1.U  // nCS always starts high
        reg_nRD := 0.U  // nRD always starts low
      }
    }
  }
  // VRAM DMA transfers at 2 MHz, even in single-speed mode.
  // This ensures that the address on the bus updates mid t-cycle
  val vramDmaCycle0 = io.busRequester === BusRequester.vramDma && io.clocker.counter8Mhz === 0.U
  when (io.busEnable && io.clocker.pulse8Mhz && vramDmaCycle0) {
    regAddressOut := io.busAddress
  }


  // Non-physical interface

  /// Whether a request is active this M-cycle
  val regActive = RegInit(false.B)
  /// Whether the next (module) cycle is the deadline for the current access
  val deadline = Wire(Bool())
  io.cartridge.reqStart := false.B
  io.cartridge.reqRom := io.busIsRom
  io.cartridge.reqWrite := io.busWrite
  io.cartridge.reqAddress := io.busAddress
  io.cartridge.reqEnd := regActive && deadline
  io.cartridge.reqDataWrite := io.busDataWrite

  when (io.clocker.enable && regActive && deadline) {
    regActive := false.B
  }
  when (io.busEnable && (io.clocker.tCycle === 0.U || vramDmaCycle0)) {
    regActive := true.B

    when (!regActive && !reset.asBool) {
      // Ensure that regStart isn't asserted for multiple cycles if
      // clocker.enable is false. If we depended directly on clocker.enable,
      // it would cause a combinatorial cycle in the outer modules.
      io.cartridge.reqStart := true.B
    }
  }

  deadline := io.lastClockCycle
  when (io.busRequester === BusRequester.vramDma) {
    deadline := io.clocker.counter8Mhz === 3.U
  }
}
