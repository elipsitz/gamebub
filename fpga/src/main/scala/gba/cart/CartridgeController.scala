package gba.cart

import chisel3._
import chisel3.util._
import gba.cart.CartridgeController.State
import gba.{MmioMap, MmioTarget}
import gba.mem.TargetInterface
import lib.log.Logger

object CartridgeController {
  object State extends ChiselEnum {
    val Idle = Value
    /// nCS goes low
    ///   Non-Seq: Can be extended by wait state
    val RomStage0 = Value
    /// nCS still low, nRD/nWR goes low (WS
    ///   Non-Seq: Can be extended by wait state
    ///       Seq: Can be extended by wait state
    val RomStage1 = Value
    /// nRD/nWR goes high
    ///  * if there's a request:
    //       start it (nextState: Rom1), keep nCS low.
    ///  * if no request, nCS goes high too
    val RomStage2 = Value

    /// nCS2 goes low
    /// ADDR put on bus
    /// For write, DATA put on bus
    /// On the *falling edge*: nRD/nRW goes low
    val RamStage0 = Value
    /// nCS2, nRD/nWR still low
    /// Can be extended by wait state
    val RamStage1 = Value
    /// nRD/nRW goes high
    ///  * if there's a request:
    ///      start it (next state: Rom0), keep nCS2 low
    ///  * if no request: on the *falling edge* nCS2 goes high.
    val RamStage2 = Value
  }
}

class WaitstateControl extends Bundle {
  // Cartridge type (read-only): false for GBA, true for CGB
  val isCgbCart = Bool()
  // Enable prefetch buffer
  // TODO: implement
  val prefetch = Bool()
  val _unused = UInt(1.W)
  // Phi clock output (Disable, 4 MHz, 8 MHz, 16 MHz)
  val phi = UInt(2.W)
  // ROM 2 wait states (sequential): 8, 1
  val ws2Next = UInt(1.W)
  // ROM 2 wait states (non-sequential): 4, 3, 2, 8
  val ws2First = UInt(2.W)
  // ROM 1 wait states (sequential): 4, 1
  val ws1Next = UInt(1.W)
  // ROM 1 wait states (non-sequential): 4, 3, 2, 8
  val ws1First = UInt(2.W)
  // ROM 0 wait states (sequential): 2, 1
  val ws0Next = UInt(1.W)
  // ROM 0 wait states (non-sequential): 4, 3, 2, 8
  val ws0First = UInt(2.W)
  // SRAM wait states: 4, 3, 2, 8
  val sram = UInt(2.W)
}

/*
 * Module that interacts with the physical cartridge bus.
 *
 * Handles control signals, wait states, bursts, etc.
 */
class CartridgeController extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())

    /// Cartridge interface
    val cartridge = new CartridgeInterface

    // MMIO interface for WAITCNT (cartridge waitstate control and prefetch buffer)
    val mmio = new MmioTarget()
    val prefetchEnabled = Output(Bool())
    val abortRequest = Input(Bool())

    // Memory bus target interfaces
    // The ROM interface is from the prefetch controller, and is halfword addressed.
    val busTargetRom = new TargetInterface(16.W)
    val busTargetRomRegion = Input(Vec(3, Bool()))
    val busTargetRam = new TargetInterface(8.W)
  })
  val logger = Logger("cart", enable = io.enable)

  val state = RegInit(State.Idle)
  val regReadData = Reg(UInt(16.W))
  val isRequestDone = WireDefault(false.B)
  val regWaitControl = RegInit(0.U.asTypeOf(new WaitstateControl))
  val waitRomFirst = Seq(regWaitControl.ws0First, regWaitControl.ws1First, regWaitControl.ws2First)
  val waitRomNext = Seq(regWaitControl.ws0Next, regWaitControl.ws1Next, regWaitControl.ws2Next)

  io.mmio <> MmioMap(
    // WAITCNT
    // TODO make sure Cartridge type always reads as 0
    0x204 -> MmioMap.Entry.rw(regWaitControl),
  )
  io.prefetchEnabled := regWaitControl.prefetch

  // Default target bus state
  io.busTargetRom.done := isRequestDone
  io.busTargetRom.dataRead := regReadData
  io.busTargetRam.done := false.B
  io.busTargetRam.dataRead := regReadData

  // Control signals come directly from registers to avoid glitches.
  // TODO: see if ADLoOut / AHiOut can also come from registers
  val reg_nCS = RegInit(1.U(1.W))
  val reg_nRD = RegInit(1.U(1.W))
  val reg_nWR = RegInit(1.U(1.W))
  val reg_nCS2 = RegInit(1.U(1.W))

  // Cartridge port
  io.cartridge.phi := 0.U  // TODO
  io.cartridge.nWR := reg_nWR
  io.cartridge.nRD := reg_nRD
  io.cartridge.nCS := reg_nCS
  io.cartridge.ADLoOut := DontCare
  io.cartridge.ADLoDir := 0.U
  io.cartridge.AHiOut := DontCare
  io.cartridge.AHiDir := 0.U
  io.cartridge.nCS2 := reg_nCS2
  io.cartridge.reqStart := false.B
  io.cartridge.reqRom := DontCare
  io.cartridge.reqAddress := DontCare
  io.cartridge.reqWrite := DontCare
  io.cartridge.reqEnd := false.B

  // TODO: romRequestAddress shouldn't combinatorially depend on `io.enable`, makes it hard to avoid cycles with stall

  // ROM targets
  val romRequests = io.busTargetRomRegion
  val hasRomRequest = io.busTargetRom.request
  val romRequestAddress = io.busTargetRom.address
  val romRequestWrite = io.busTargetRom.write
  val romRequestDataWrite = io.busTargetRom.dataWrite
  val romRequestSequential = io.busTargetRom.sequential
  val romRequestNextSequential = io.busTargetRom.nextSeq
  val ramTarget = io.busTargetRam
  val currentRequestPort = Reg(UInt(3.W))
  val currentAddress = Reg(UInt(24.W))
  val currentIsWrite = Reg(Bool())
  val waitCounter = Reg(UInt(3.W))
  val romAddressAtPageEnd = currentAddress(15, 0).andR  // All 1s, highest address at page end
  val doLatchReadData = Reg(Bool())


  // New requests (not burst continuations) can be accepted when idle or at the end of a burst.
  // We can do back-to-back ROM requests (and RAM requests), as long as nCS has been raised high.
  val endRomBurst = WireDefault(false.B)
  val endRamBurst = WireDefault(false.B)
  when (state === State.Idle || endRomBurst || endRamBurst) {
    when (hasRomRequest && reg_nCS === 1.U) {
      logger.debug(cf"Start Rom(${romRequests.asUInt}%b) request addr=${romRequestAddress << 1}%x wr=$romRequestWrite")
      io.cartridge.ADLoOut := romRequestAddress(15, 0)
      io.cartridge.ADLoDir := true.B
      io.cartridge.AHiOut := romRequestAddress(23, 16)
      io.cartridge.AHiDir := true.B

      io.cartridge.reqStart := true.B
      io.cartridge.reqRom := true.B
      io.cartridge.reqAddress := romRequestAddress
      io.cartridge.reqWrite := romRequestWrite

      when (io.enable) {
        state := State.RomStage0
        reg_nCS := 0.U
        currentAddress := romRequestAddress
        currentIsWrite := romRequestWrite
        currentRequestPort := romRequests.asUInt

        // Initial burst wait: 0 [extra] cycles if total waits is 2,
        // otherwise 1.
        val wait = Mux1H(romRequests, waitRomFirst)
        when (wait === 2.U) {
          waitCounter := 0.U
        } .otherwise {
          waitCounter := 1.U
        }
      }
    } .elsewhen (ramTarget.request && reg_nCS2 === 1.U) {
      logger.debug(cf"Start Ram request addr=${ramTarget.address(15, 0)}%x wr=${ramTarget.write}")

      // TODO: should we put ADDR on the bus early?
      // Note: we don't signal emuCart write until we have the write data, in RamStage0

      when (io.enable) {
        state := State.RamStage0
        reg_nCS2 := 0.U
        // XXX: nRD/nWR are supposed to go low on the *falling* edge of the next cycle
        when (ramTarget.write) {
          reg_nWR := 0.U
        } .otherwise {
          reg_nRD := 0.U
        }
        currentAddress := ramTarget.address
        currentIsWrite := ramTarget.write
      }
    } .otherwise {
      when (io.enable) {
        state := State.Idle
      }
    }
  }

  switch (state) {
    is (State.RomStage0) {
      io.cartridge.ADLoOut := currentAddress(15, 0)
      io.cartridge.ADLoDir := true.B
      io.cartridge.AHiOut := currentAddress(23, 16)
      io.cartridge.AHiDir := true.B

      val wait = Mux1H(currentRequestPort, waitRomFirst)
      when (currentIsWrite && waitCounter === 0.U && wait =/= 2.U) {
        // If this is the second cycle of a two cycle initial wait, set WDATA output.
        io.cartridge.ADLoDir := true.B
        io.cartridge.ADLoOut := romRequestDataWrite
      }

      when (io.enable) {
        waitCounter := waitCounter - 1.U
        when (waitCounter === 0.U) {
          state := State.RomStage1
          waitCounter := VecInit(1.U, 0.U, 0.U, 5.U)(wait)
          when (currentIsWrite) {
            reg_nWR := 0.U
          } .otherwise {
            reg_nRD := 0.U
          }
        }
      }
    }
    is (State.RomStage1) {
      when (currentIsWrite) {
        io.cartridge.ADLoDir := true.B
        io.cartridge.ADLoOut := romRequestDataWrite
      } .otherwise {
        io.cartridge.ADLoDir := false.B
      }
      io.cartridge.AHiOut := currentAddress(23, 16)
      io.cartridge.AHiDir := true.B

      when (io.enable) {
        waitCounter := waitCounter - 1.U
        when (waitCounter === 0.U) {
          state := State.RomStage2
          regReadData := io.cartridge.ADLoIn
          reg_nRD := 1.U
          reg_nWR := 1.U
          doLatchReadData := true.B

          when (!romRequestNextSequential || romAddressAtPageEnd) {
            // The request is definitely not being continued with a burst, put nCS back high
            reg_nCS := 1.U
          }
        }
      }
    }
    is (State.RomStage2) {
      // There's a write data hold time requirement:
      // we need to keep the write data on the bus after nWR goes high
      // (the GBA keeps it on the bus for ~1/2 cycle, and then switches to the
      //  next request's address)
      // The proper way to do this would probably be to use an actual DDR output,
      // but it's difficult (or impossible?) to time, so we'll have to hope this
      // works well enough.
      when (!io.cartridge.isEmulated && clock.asBool) {
        when (currentIsWrite) {
          io.cartridge.ADLoDir := true.B
          io.cartridge.ADLoOut := romRequestDataWrite
        } .otherwise {
          io.cartridge.ADLoDir := false.B
        }
        io.cartridge.AHiOut := currentAddress(23, 16)
        io.cartridge.AHiDir := true.B
      }
      // A16-23 are live address lines that the cartridge ROM uses directly: while nCS is still
      // low, keep driving them for the whole cycle (like a GBA). Releasing them for half a cycle
      // let them float during the access, which can corrupt reads from slower or larger ROMs.
      // (Once nCS is high, the second half of the cycle can carry the next request's address.)
      when (!io.cartridge.isEmulated && reg_nCS === 0.U) {
        io.cartridge.AHiOut := currentAddress(23, 16)
        io.cartridge.AHiDir := true.B
      }


      when (io.cartridge.isEmulated && doLatchReadData) {
        io.busTargetRom.dataRead := io.cartridge.ADLoIn
        io.cartridge.reqEnd := true.B

        when (io.enable) {
          regReadData := io.cartridge.ADLoIn
          doLatchReadData := false.B
        }
      }
      isRequestDone := true.B

      when (reg_nCS === 1.U) {
        // Bus doesn't have a sequential request (for any target). End burst.
        endRomBurst := true.B
      } .elsewhen (hasRomRequest && romRequestSequential) {
        // Starting a new, sequential request, nCS was kept low.
        io.cartridge.reqStart := true.B
        io.cartridge.reqRom := true.B
        io.cartridge.reqAddress := romRequestAddress
        io.cartridge.reqWrite := romRequestWrite

        when (io.enable) {
          state := State.RomStage1
          when (romRequestWrite) {
            reg_nWR := 0.U
          } .otherwise {
            reg_nRD := 0.U
          }
          currentAddress := romRequestAddress
          currentIsWrite := romRequestWrite

          val wait = Mux1H(currentRequestPort, waitRomNext)
          when (wait === 1.U) {
            waitCounter := 0.U  // 1 wait state
          } .otherwise {
            waitCounter := Mux1H(currentRequestPort, Seq(1.U, 3.U, 7.U))
          }
        }
      } .elsewhen (!romRequestNextSequential || (hasRomRequest && !romRequestSequential)) {
        // Time to end the request
        endRomBurst := true.B
        when (io.enable) {
          reg_nCS := 1.U
        }
      } .otherwise {
        // Still in a burst (keep nCS high), but haven't yet received a request.
      }
    }
    is (State.RamStage0) {
      // Now that we have write data, signal the access.
      // TODO: Note that reqStart will be asserted for multiple cycles if io.enable is false
      io.cartridge.reqStart := true.B
      io.cartridge.reqRom := false.B
      io.cartridge.reqAddress := currentAddress(15, 0)
      io.cartridge.reqWrite := currentIsWrite

      io.cartridge.ADLoOut := currentAddress(15, 0)
      io.cartridge.ADLoDir := true.B
      when (currentIsWrite) {
        io.cartridge.AHiDir := true.B
        io.cartridge.AHiOut := ramTarget.dataWrite
      }

      when (io.enable) {
        state := State.RamStage1
        waitCounter := VecInit(2.U, 1.U, 0.U, 6.U)(regWaitControl.sram)
      }
    }
    is (State.RamStage1) {
      io.cartridge.ADLoOut := currentAddress(15, 0)
      io.cartridge.ADLoDir := true.B
      when (currentIsWrite) {
        io.cartridge.AHiDir := true.B
        io.cartridge.AHiOut := ramTarget.dataWrite
      }

      io.cartridge.reqEnd := waitCounter === 0.U
      when (io.enable) {
        waitCounter := waitCounter - 1.U
        when (waitCounter === 0.U) {
          state := State.RamStage2
          regReadData := io.cartridge.AHiIn
          reg_nWR := 1.U
          reg_nRD := 1.U
          // XXX: nCS2 should go back high on the *falling* edge of the next cycle
          // TODO: use requestSplitForceNextSequential to determine if nCS2 goes back high
          reg_nCS2 := 1.U
        }
      }
    }
    is (State.RamStage2) {
      ramTarget.done := true.B
      // nRD/NRW went back high
      when (ramTarget.request && ramTarget.sequential) {
        logger.debug(cf"Continue ram request")
        // Starting a new "sequential" request -- nCS2 stays low
        // XXX: in a burst, the next ADDR is put on the bus in the *falling* edge of this cycle
        // ... but we don't do that here. It's probably fine to just put it on the next rising
        // edge, because that's the timing of the first request in a burst anyway.

        when (io.enable) {
          state := State.RamStage0
          when (ramTarget.write) {
            reg_nWR := 0.U
          } .otherwise {
            reg_nRD := 0.U
          }
          currentAddress := ramTarget.address
          currentIsWrite := ramTarget.write
        }
      } .otherwise {
        // Not getting another request this cycle, end burst.

        logger.debug(cf"End ram request")
        endRamBurst := true.B
      }
    }
  }

  // When a request is aborted, immediately move to idle state (and adjust signals).
  // It is assumed that the requester will ignore any data returned.
  when (io.enable && io.abortRequest) {
    state := State.Idle
    reg_nCS := 1.U
    reg_nCS2 := 1.U
    reg_nRD := 1.U
    reg_nWR := 1.U
  }
}
