package gba.cart.emu

import chisel3._
import chisel3.util._
import gba.cart.emu.FlashBackup.State
import lib.log.Logger
import lib.mem.MemoryInterface

object FlashBackup {
  object State extends ChiselEnum {
    val Ready = Value
    val Setup1 = Value
    val Setup2 = Value
    val BankSwap = Value
    val PrepareWrite = Value
    val DoWrite = Value
  }
}

class FlashBackup extends Module {
  val io = IO(new Bundle {
    val ramEnable = Input(Bool())
    val ramAddress = Input(UInt(16.W))
    val ramIsWrite = Input(Bool())
    val ramDataRead = Output(UInt(8.W))
    val ramDataWrite = Input(UInt(8.W))
    val ramReqEnd = Input(Bool())

    val stall = Output(Bool())

    val backup = Flipped(new MemoryInterface(addressWidth = 17, dataWidth = 8))
    val size = Input(UInt(1.W))
  })
  val logger = Logger("cart.emu.flash")

  val regState = RegInit(State.Ready)
  val regBank = RegInit(0.U(1.W))
  val regModeChipId = RegInit(false.B)
  val regModeErase = RegInit(false.B)
  val regReadBusy = RegInit(false.B)
  val regReadData = Reg(UInt(8.W))
  val regAddress = Reg(UInt(17.W))
  val regEraseBusy = RegInit(false.B)
  val regEraseFull = Reg(Bool())

  io.stall := false.B
  io.backup.enable := false.B
  io.backup.address := regAddress
  io.backup.write := DontCare
  io.backup.writeStrobe := 1.U
  io.backup.dataWrite := io.ramDataWrite
  io.ramDataRead := regReadData

  when (io.ramEnable && io.ramIsWrite) {
    logger.debug(cf"write addr=${io.ramAddress}%x data=${io.ramDataWrite}%x | state=${regState}")
  }

  // Handle reading
  when (io.ramEnable && !io.ramIsWrite) {
    when (regModeChipId && io.ramAddress < 2.U) {
      when (io.size === 0.U) {
        // 64 KiB (Panasonic)
        regReadData := Mux(io.ramAddress(0) === 0.U, 0x32.U, 0x1B.U)
      } .otherwise {
        // 128 KiB (Sanyo)
        regReadData := Mux(io.ramAddress(0) === 0.U, 0x62.U, 0x13.U)
      }
    } .elsewhen (regEraseBusy) {
      // GBA software looks for 0xFF to know if the erase is complete.
      // But perhaps this should only trigger when reading a sector that is being
      // erased, and other read should be unaffected.
      regReadData := 0.U
    } .otherwise {
      regReadBusy := true.B
      regAddress := Cat(regBank, io.ramAddress)
    }
  }
  when (regEraseBusy) {
    io.backup.enable := true.B
    io.backup.write := true.B
    io.backup.dataWrite := 0xFF.U
    when (io.backup.done) {
      val nextAddress = regAddress + 1.U
      regAddress := nextAddress

      when (regEraseFull && nextAddress === 0.U) {
        logger.info(cf"Finished erase full")
        regEraseBusy := false.B
      }
      when (!regEraseFull && nextAddress(11, 0) === 0.U) {
        logger.info(cf"Finished erase sector")
        regEraseBusy := false.B
      }
    }
    when (io.ramReqEnd) {
      logger.warn("Flash erase stall")
    }
  } .elsewhen (regReadBusy) {
    io.backup.enable := true.B
    io.backup.write := false.B
    when (io.backup.done) {
      logger.debug(cf"Flash read done: addr=0x${regAddress}%x data=0x${io.backup.dataRead}%x")
      regReadBusy := false.B
      regReadData := io.backup.dataRead
    }
    when (io.ramReqEnd) {
      logger.warn("Flash read stall")
      io.stall := true.B
    }
  }

  // Handle writes in various states.
  switch (regState) {
    is (State.Ready) {
      when (io.ramEnable && io.ramIsWrite) {
        regState := State.Ready
        when (io.ramAddress === 0x5555.U && io.ramDataWrite === 0xAA.U) {
          regState := State.Setup1
        }
      }
    }
    is (State.Setup1) {
      when (io.ramEnable && io.ramIsWrite) {
        regState := State.Ready
        when (io.ramAddress === 0x2AAA.U && io.ramDataWrite === 0x55.U) {
          regState := State.Setup2
        }
      }
    }
    is (State.Setup2) {
      when (io.ramEnable && io.ramIsWrite) {
        regState := State.Ready
        when (io.ramAddress === 0x5555.U) {
          switch (io.ramDataWrite) {
            is (0x90.U) {
              // Enter chip ID mode
              regModeChipId := true.B
              logger.info("Enter chip id")
            }
            is (0xF0.U) {
              // Exit chip ID mode
              regModeChipId := false.B
              logger.info("Exit chip id")
            }
            is (0x80.U) {
              // Prepare to receive erase
              regModeErase := true.B
            }
            is (0x10.U) {
              // Erase entire chip
              when (regModeErase) {
                logger.info("Erase chip")
                regModeErase := false.B
                regAddress := 0.U
                regEraseBusy := true.B
                regEraseFull := true.B
              }
            }
            is (0xA0.U) {
              // Prepare to write single data byte
              regState := State.PrepareWrite
            }
            is (0xB0.U) {
              // Swap memory bank
              when (io.size === 1.U) {
                regState := State.BankSwap
              }
            }
          }
        }
        when (regModeErase && io.ramDataWrite === 0x30.U) {
          // Erase a 4KB sector
          val sector = io.ramAddress(15, 12)
          logger.info(cf"Erase sector ${sector}")
          regModeErase := false.B
          regAddress := Cat(regBank, sector, 0.U(12.W))
          regEraseBusy := true.B
          regEraseFull := false.B
        }
      }
    }
    is (State.PrepareWrite) {
      when (io.ramEnable && io.ramIsWrite) {
        regAddress := Cat(regBank, io.ramAddress)
        regState := State.DoWrite
      }
    }
    is (State.DoWrite) {
      // Actually perform the write.
      io.backup.enable := true.B
      io.backup.write := true.B
      when (io.backup.done) {
        logger.debug(cf"Flash write done: addr=0x${regAddress}%x data=0x${io.ramDataWrite}%x")
        regState := State.Ready
      }
      when (io.ramReqEnd) {
        logger.warn("Flash write stall")
        io.stall := true.B
      }
    }
    is (State.BankSwap) {
      when (io.ramEnable && io.ramIsWrite) {
        regState := State.Ready
        when (io.ramAddress === 0.U && io.size === 1.U) {
          regBank := io.ramDataWrite(0)
          logger.debug(cf"Bank swap: ${io.ramDataWrite(0)}")
        }
      }
    }
  }
}
