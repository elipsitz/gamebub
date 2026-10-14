package gba

import chisel3._
import chisel3.util._
import gba.mem.{BusAccessWidth, BusInterface}
import lib.log.Logger

object DmaAddressControl extends ChiselEnum {
  val increment = Value
  val decrement = Value
  val fixed = Value
  val reload = Value
}

object DmaStartControl extends ChiselEnum {
  val immediate = Value
  val vblank = Value
  val hblank = Value
  val special = Value
}

class DmaControl extends Bundle {
  val enable = Bool()
  val irq = Bool()
  val startControl = DmaStartControl()
  val cartridgeDrq = Bool()
  val sizeWord = Bool()
  val repeat = Bool()
  val sourceControl = DmaAddressControl()
  val destControl = DmaAddressControl()
  val _padding = UInt(5.W)
}

class Dma extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val mmio = new MmioTarget()

    val irq = Output(Vec(4, Bool()))

    val triggerVblank = Input(Bool())
    val triggerHblank = Input(Bool())
    val triggerVideo = Input(Bool())
    val triggerFifo = Input(Vec(2, Bool()))
    val stopVideo = Input(Bool())

    val busInitiator = Vec(4, new BusInterface)
  })
  private val logger = Logger("dma", enable = io.enable)

  val regConfigSource = Seq(RegInit(0.U(27.W)), RegInit(0.U(28.W)), RegInit(0.U(28.W)), RegInit(0.U(28.W)))
  val regConfigDest = Seq(RegInit(0.U(27.W)), RegInit(0.U(27.W)), RegInit(0.U(27.W)), RegInit(0.U(28.W)))
  val regConfigCount = Seq(RegInit(0.U(14.W)), RegInit(0.U(14.W)), RegInit(0.U(14.W)), RegInit(0.U(16.W)))
  val regConfigControl = Seq.fill(4)(RegInit(0.U.asTypeOf(new DmaControl)))

  io.mmio <> MmioMap.fromSeq(
    (0 until 4).flatMap(i => Seq(
      (0xB0 + (i * 12)) -> MmioMap.Entry.w(regConfigSource(i)),
      (0xB4 + (i * 12)) -> MmioMap.Entry.w(regConfigDest(i)),
      (0xB8 + (i * 12)) -> MmioMap.Entry(
        MmioMap.ReadFn(0.U(8.W), regConfigControl(i)),
        MmioMap.WriteFn(regConfigCount(i), regConfigControl(i))
      )
    ))
  )

  // Whether any channel is active and in the "store" phase.
  val isAnyChannelStoring = WireDefault(false.B)

  for (i <- 0 until 4) {
    val configSource = regConfigSource(i)
    val configDest = regConfigDest(i)
    val configCount = regConfigCount(i)
    val control = regConfigControl(i)
    val bus = io.busInitiator(i)

    val isAudioFifo = (i == 1 || i == 2).B && control.startControl === DmaStartControl.special
    val isSizeWord = control.sizeWord || isAudioFifo

    val active = RegInit(false.B)
    val regSource = Reg(configSource.cloneType)
    val regDest = Reg(configDest.cloneType)
    val regCount = Reg(configCount.cloneType)
    val regInitial = Reg(Bool())  // Whether this is the initial load-store cycle.
    val regAccessedCart = Reg(Bool()) // Whether the cartridge has already been accessed this cycle
    val regStage = Reg(UInt(1.W))
    val dataLatch = Reg(UInt(32.W))
    val regSourceAddressHalfword = Reg(UInt(1.W))

    val busActive = WireDefault(false.B)
    io.irq(i) := false.B
    bus.WRITE := DontCare
    bus.SIZE := Mux(isSizeWord, BusAccessWidth.Word, BusAccessWidth.Halfword)
    bus.PROT.data := true.B
    bus.PROT.privileged := false.B
    bus.LOCK := false.B
    bus.ADDR := DontCare
    bus.MREQ := busActive
    bus.SEQ := regAccessedCart
    bus.WDATA := dataLatch

    // Latching config
    val prevEnabled = RegEnable(control.enable, io.enable)
    val justEnabled = control.enable && !prevEnabled
    val addressMask = Mux(isSizeWord, "b00".U(2.W), "b10".U(2.W))
    when (io.enable && justEnabled) {
      logger.info(cf"${i}: enabled: ${control}")
      // Mask off lower bits of address depending on size
      regSource := Cat(configSource(configSource.getWidth - 1, 2), configSource(1, 0) & addressMask)
      regDest := Cat(configDest(configDest.getWidth - 1, 2), configDest(1, 0) & addressMask)

      when (isAudioFifo) {
        // Special audio FIFO handling
        // Forces count=4, size=word, destControl=fixed
        regCount := 4.U
      } .otherwise {
        regCount := configCount
      }
    }

    // Channel activation
    val activateImm = (control.startControl === DmaStartControl.immediate) && justEnabled
    val activateHblank = (control.startControl === DmaStartControl.hblank) && io.triggerHblank
    val activateVblank = (control.startControl === DmaStartControl.vblank) && io.triggerVblank
    val activateSpecial = WireDefault(false.B)
    if (i == 1 || i == 2) {
      activateSpecial := control.startControl === DmaStartControl.special && io.triggerFifo(i - 1)
    } else if (i == 3) {
      activateSpecial := control.startControl === DmaStartControl.special && io.triggerVideo
      when (control.startControl === DmaStartControl.special && io.stopVideo) {
        control.enable := false.B
      }
    }
    when (io.enable) {
      when (!active && control.enable && (activateImm || activateHblank || activateVblank || activateSpecial)) {
        logger.info(cf"${i}: activate")
        active := true.B
        regInitial := true.B
        regStage := 0.U
        regAccessedCart := false.B
      }
    }

    // Each DMA channel can only be pre-empted during the "load" phase -- that is, once a DMA channel
    // submits a load to the bus, it won't be pre-empted until after it completes the store.
    //
    // This is done slightly messily here: if any channel is storing, a DMA can't start,
    // unless it's *us* who is storing, or we're past the initial load/store.
    when (active && regStage === 1.U) {
      isAnyChannelStoring := true.B
    }
    val isNotBlocked = !isAnyChannelStoring || regStage === 1.U || !regInitial

    // Run DMA
    when (io.enable && active && isNotBlocked) {
      when (regStage === 0.U) {
        val complete = regCount === 0.U && !regInitial
        // Begin Load (if not end)
        busActive := !complete
        bus.ADDR := regSource
        bus.WRITE := false.B
        if (regSource.getWidth >= 28) {
          when (regSource(27)) {
            // TODO: this should probably only be set on bus.CLKEN
            regAccessedCart := true.B
          }
        }
        // Complete Store (if not initial)
        regSourceAddressHalfword := regSource(1)

        // Check if the DMA is complete.
        when (bus.CLKEN) {
          when (complete) {
            logger.info(cf"${i}: complete")
            io.irq(i) := control.irq
            active := false.B
            when (control.repeat && isAudioFifo) {
              // Audio fifo is forced at 4 words and fixed destination
              regCount := 4.U
            } .elsewhen (control.repeat && control.startControl =/= DmaStartControl.immediate) {
              regCount := configCount
              when (control.destControl === DmaAddressControl.reload) {
                regDest := Cat(configDest(configDest.getWidth - 1, 2), configDest(1, 0) & addressMask)
              }
            } .otherwise {
              control.enable := false.B
            }
          } .otherwise {
            logger.debug(cf"${i}: load @ 0x${regSource}%x")
            regStage := 1.U
            regCount := regCount - 1.U

            switch (control.sourceControl) {
              is (DmaAddressControl.increment) {
                regSource := regSource + Mux(isSizeWord, 4.U, 2.U)
              }
              is (DmaAddressControl.decrement) {
                regSource := regSource - Mux(isSizeWord, 4.U, 2.U)
              }
            }
          }
        }
      } .otherwise {
        // Complete Load
        when (bus.CLKEN) {
          when (isSizeWord) {
            dataLatch := bus.RDATA
          } .otherwise {
            dataLatch := Fill(2, Mux(regSourceAddressHalfword.asBool, bus.RDATA(31, 16), bus.RDATA(15, 0)))
          }
        }

        // Begin Store
        busActive := true.B
        bus.ADDR := regDest
        bus.WRITE := true.B
        if (regDest.getWidth >= 28) {
          when (regDest(27)) {
            // TODO: this should probably only be set on bus.CLKEN
            regAccessedCart := true.B
          }
        }

        when (bus.CLKEN) {
          logger.debug(cf"${i}: store @ 0x${regDest}%x  (data = 0x${bus.RDATA}%x)")
          regStage := 0.U
          regInitial := false.B

          when (!isAudioFifo) {
            switch(control.destControl) {
              is(DmaAddressControl.increment, DmaAddressControl.reload) {
                regDest := regDest + Mux(isSizeWord, 4.U, 2.U)
              }
              is(DmaAddressControl.decrement) {
                regDest := regDest - Mux(isSizeWord, 4.U, 2.U)
              }
            }
          }
        }
      }
    }
  }
}