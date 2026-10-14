package gba.link

import chisel3._
import chisel3.util._
import gba.link.Link.JoybusCommand
import gba.{MMIO, MmioMap, MmioTarget}
import lib.log.Logger

class Link extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())
    val mmio = new MmioTarget()
    val irq = Output(Bool())

    val port = new Link.Interface
  })
  val logger = Logger("link", enable = io.enable)

  val mode = Wire(Link.Mode.Type())
  val prevMode = RegEnable(mode, io.enable)
  val prevPortIn = RegEnable(io.port.in, io.enable)
  /// Whether bit 7 of SIOCNT has been written this cycle (only for Normal and Multi mode).
  val siocntStartSet = WireDefault(false.B)
  val siocntStartUnset = WireDefault(false.B)
  val regSiocntStart = RegEnable(siocntStartSet, io.enable)
  /// SIOCNT read value (mode-dependent), low 8-bits. Upper 8 bits are all R/W and shared
  val siocntReadValueLo = WireDefault(0.U(8.W))

  // RCNT registers
  val regControlMode = RegInit(0.U(2.W))
  val regGpioPinOut = RegInit(0.U(4.W))
  val regGpioPinDir = RegInit(0.U(4.W))
  val regGpioInterrupt = RegInit(false.B)
  // SIOCNT register
  val regSiocnt = RegInit(0.U(15.W))
  // 0x12A: SIODATA8 / SIOMLT_SEND
  val regDataA = RegInit(0.U(16.W))
  // 0x120: SIODATA32_L / SIOMULTI0
  val regDataB0 = RegInit(0.U(16.W))
  // 0x122: SIODATA32_H / SIOMULTI1
  val regDataB1 = RegInit(0.U(16.W))
  // 0x124: SIOMULTI2
  val regDataB2 = RegInit(0.U(16.W))
  // 0x126: SIOMULTI3
  val regDataB3 = RegInit(0.U(16.W))
  // 0x140: JOYCNT
  val regJoyCntReset = RegInit(false.B)
  val regJoyCntReceive = RegInit(false.B)
  val regJoyCntSend = RegInit(false.B)
  val regJoyCntInterrupt = RegInit(false.B)
  // 0x150: JOY_RECV
  val regJoyRecv = RegInit(0.U(32.W))
  // 0x154: JOY_TRANS
  val regJoyTrans = RegInit(0.U(32.W))
  // 0x158: JOYSTAT
  val regJoyStatRx = RegInit(false.B)
  val regJoyStatTx = RegInit(false.B)
  val regJoyStatFlags = RegInit(0.U(2.W))
  val joyStatRead = Cat(
    0.U(2.W),
    regJoyStatFlags,
    regJoyStatTx,
    0.U(1.W),
    regJoyStatRx,
    0.U(1.W),
  )

  io.irq := false.B
  io.mmio <> MmioMap(
    0x120 -> MmioMap.Entry.rw16(regDataB0, regDataB1),
    0x124 -> MmioMap.Entry.rw16(regDataB2, regDataB3),
    // SIOCNT
    0x128 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(regDataA, 0.U(1.W), regSiocnt(14, 8), siocntReadValueLo)),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          regSiocnt := MMIO.mask(regSiocnt, data(15, 0), mask(1, 0))
          regDataA := MMIO.mask(regDataA, data(31, 16), mask(3, 2))
          siocntStartSet := mask(0) && data(7)
          siocntStartUnset := mask(0) && !data(7)
        }
      })
    ),
    // RCNT
    0x134 -> MmioMap.Entry(
      MmioMap.ReadFn({
        val out = Wire(new Link.RegisterRcnt)
        out.pinData := io.port.in.asUInt
        out.pinDir := regGpioPinDir
        out.interrupt := regGpioInterrupt
        out._unused := 0.U
        out.mode := regControlMode
        out
      }),
      MmioMap.WriteFn((enable, rawData, mask) => {
        val data = rawData.asTypeOf(new Link.RegisterRcnt)
        when (enable && mask(0)) {
          regGpioPinOut := data.pinData
          regGpioPinDir := data.pinDir
        }
        when (enable && mask(1)) {
          regGpioInterrupt := data.interrupt
          regControlMode := data.mode
        }
      })
    ),
    // JOYCNT
    0x140 -> MmioMap.Entry(
      MmioMap.ReadFn(Cat(
        regJoyCntInterrupt,
        0.U(3.W),
        regJoyCntSend,
        regJoyCntReceive,
        regJoyCntReset,
      )),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable && mask(0)) {
          when (data(0)) {
            regJoyCntReset := false.B
          }
          when (data(1)) {
            regJoyCntReceive := false.B
          }
          when (data(2)) {
            regJoyCntSend := false.B
          }
          regJoyCntInterrupt := data(6)
        }
      })
    ),
    // JOY_RECV
    0x150 -> MmioMap.Entry(
      MmioMap.ReadFn(enable => {
        when (enable) {
          regJoyStatRx := false.B
        }
        (regJoyRecv, true.B)
      }),
      MmioMap.WriteFn(regJoyRecv),
    ),
    // JOY_TRANS
    0x154 -> MmioMap.Entry(
      MmioMap.ReadFn(regJoyTrans),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable) {
          regJoyTrans := MMIO.mask(regJoyTrans, data, mask)
          regJoyStatTx := true.B
        }
      })
    ),
    // JOYSTAT
    0x158 -> MmioMap.Entry(
      MmioMap.ReadFn(joyStatRead),
      MmioMap.WriteFn((enable, data, mask) => {
        when (enable && mask(0)) {
          regJoyStatFlags := data(5, 4)
        }
      })
    ),
  )

  // Mode selection
  when (regControlMode(1) === 0.U) {
    when (regSiocnt(13) === 0.U) {
      mode := Link.Mode.Normal
    } .otherwise {
      when (regSiocnt(12) === 0.U) {
        mode := Link.Mode.Multi
      } .otherwise {
        mode := Link.Mode.Uart
      }
    }
  } .otherwise {
    when (regControlMode(0) === 0.U) {
      mode := Link.Mode.Gpio
    } .otherwise {
      mode := Link.Mode.Joybus
    }
  }

  // Stubbed: all inputs (high-z)
  io.port.out := DontCare
  io.port.out.sd := false.B
  io.port.dir.si := false.B
  io.port.dir.so := false.B
  io.port.dir.sd := true.B
  io.port.dir.sc := false.B

  val uartTimer = Module(new UartTimer)
  uartTimer.io.enable := io.enable
  uartTimer.io.baudrate := DontCare

  switch (mode) {
    is (Link.Mode.Normal) {
      handleNormal()
    }
    is (Link.Mode.Multi) {
      handleMulti()
    }
    is (Link.Mode.Gpio) {
      handleGpio()
    }
    is (Link.Mode.Joybus) {
      handleJoybus()
    }
    // TODO support UART mode
  }

  private def handleGpio(): Unit = {
    io.port.out := regGpioPinOut.asTypeOf(new Link.Ports)
    io.port.dir := regGpioPinDir.asTypeOf(new Link.Ports)

    when (regGpioInterrupt && !io.port.in.si && prevPortIn.si) {
      // SI falling edge interrupt
      io.irq := true.B
    }
  }

  private def handleNormal(): Unit = {
    val regBusy = RegInit(false.B)
    val regMasterHold = Reg(Bool())
    val regBitCounter = Reg(UInt(6.W))
    val regClockCounter = Reg(UInt(5.W))
    val regClockOut = Reg(Bool())
    val regDataOut = Reg(Bool())
    val regShiftIn = Reg(UInt(32.W))
    val regShiftOut = Reg(UInt(32.W))
    when (io.enable && prevMode =/= Link.Mode.Normal) {
      regBusy := false.B
    }

    // SIOCNT
    val siocntLo = regSiocnt.asTypeOf(new Link.NormalSiocntLo)
    val siocntLoRead = Wire(new Link.NormalSiocntLo)
    siocntReadValueLo := siocntLoRead.asUInt
    siocntLoRead.busy := regBusy
    siocntLoRead._unused := 0.U
    siocntLoRead.soIdle := siocntLo.soIdle
    siocntLoRead.si := io.port.in.si
    siocntLoRead.fastClock := siocntLo.fastClock
    siocntLoRead.master := siocntLo.master
    val transfer32 = regSiocnt(12)
    val interruptEnable = regSiocnt(14)
    val isMaster = siocntLo.master
    val fastClock = siocntLo.fastClock

    io.port.dir.si := false.B
    io.port.dir.so := true.B
    io.port.dir.sd := true.B
    io.port.dir.sc := siocntLo.master
    io.port.out.si := DontCare
    io.port.out.so := Mux(regBusy, regDataOut, siocntLo.soIdle)
    io.port.out.sd := false.B // Always SD = low
    io.port.out.sc := Mux(regBusy, regClockOut, true.B)

    when (regSiocntStart && !regBusy) {
      logger.info(cf"Normal start, master=${isMaster}")
      regBusy := true.B
      regMasterHold := false.B
      regBitCounter := Mux(transfer32, 32.U, 8.U)
      regClockCounter := 0.U
      regClockOut := true.B
      regDataOut := siocntLo.soIdle
      when (transfer32) {
        regShiftOut := Cat(regDataB1, regDataB0)
      } .otherwise {
        regShiftOut := regDataA(7, 0)
      }
    }

    when (io.enable && regBusy) {
      val clockRising = WireDefault(false.B)
      val clockFalling = WireDefault(false.B)

      // Control master clock
      when (isMaster) {
        val nextClockCounter = regClockCounter +& 1.U
        regClockCounter := nextClockCounter
        // Tick every clock *edge* (rising or falling)
        val tick = Mux(fastClock, nextClockCounter(2) =/= regClockCounter(2), nextClockCounter(5))
        when (tick) {
          logger.debug("Normal master clock tick")
          regClockOut := !regClockOut
          when (regClockOut) {
            clockFalling := true.B
          } .otherwise {
            clockRising := true.B
          }
        }
      } .otherwise {
        when (io.port.in.sc && !prevPortIn.sc) {
          clockRising := true.B
        }
        when (!io.port.in.sc && prevPortIn.sc) {
          clockFalling := true.B
        }
      }

      when (clockFalling) {
        // Shift out
        logger.debug("Falling clock")
        regDataOut := Mux(transfer32, regShiftOut(31), regShiftOut(7))
        regShiftOut := regShiftOut << 1
      }

      when (clockRising) {
        // Shift in
        logger.debug("Rising clock")
        val nextRegShiftIn = Cat(regShiftIn, io.port.in.si)
        regShiftIn := nextRegShiftIn

        val nextBitCounter = regBitCounter - 1.U
        regBitCounter := nextBitCounter
        when (nextBitCounter === 0.U) {
          logger.info("Normal transfer done")
          when (isMaster) {
            // Master needs to hold data for another half cycle or so for slave to read.
            regMasterHold := true.B
          } .otherwise {
            regBusy := false.B
            io.irq := interruptEnable
          }
          when (transfer32) {
            regDataB0 := nextRegShiftIn(15, 0)
            regDataB1 := nextRegShiftIn(31, 16)
          } .otherwise {
            regDataA := nextRegShiftIn(7, 0)
          }
        }
      }

      when (regMasterHold) {
        regClockOut := true.B
        when (clockFalling) {
          logger.info("Master hold complete")
          regBusy := false.B
          io.irq := interruptEnable
        }
      }

      when (siocntStartUnset) {
        logger.info("Normal transfer cancelled")
        regBusy := false.B
      }
    }
  }

  private def handleMulti(): Unit = {
    val rxDataRegs = Seq(regDataB0, regDataB1, regDataB2, regDataB3)

    // Whether we are acting as a master (always false in Idle)
    val regMaster = RegInit(false.B)
    val regMyId = RegInit(0.U(2.W))
    val regError = RegInit(false.B)
    val regState = RegInit(Link.MultiState.Idle)
    val regBusy = RegInit(false.B)
    when (io.enable && prevMode =/= Link.Mode.Multi) {
      regState := Link.MultiState.Idle
      regBusy := false.B
      regError := false.B
    }

    // SIOCNT
    val siocntLo = regSiocnt.asTypeOf(new Link.MultiSiocntLo)
    val siocntLoRead = Wire(new Link.MultiSiocntLo)
    siocntReadValueLo := siocntLoRead.asUInt
    siocntLoRead.baud := siocntLo.baud
    siocntLoRead.si := RegEnable(io.port.in.si, io.enable)
    siocntLoRead.sd := RegEnable(io.port.in.sd, io.enable)
    siocntLoRead.id := regMyId
    siocntLoRead.error := regError
    siocntLoRead.busy := regBusy
    val interruptEnable = regSiocnt(14)

    uartTimer.io.baudrate := siocntLo.baud

    io.port.dir.so := true.B  // Always Output
    io.port.dir.si := false.B // Always Input
    io.port.dir.sd := false.B // Default: input
    io.port.dir.sc := regMaster
    io.port.out.so := true.B  // Default: high
    io.port.out.si := DontCare
    io.port.out.sd := DontCare
    io.port.out.sc := false.B

    val regPeerId = Reg(UInt(2.W))
    val regTxBuffer = Reg(UInt(18.W))
    val regRxBuffer = Reg(UInt(18.W))
    val regPulseCounter = Reg(UInt(5.W))
    val regWaitCounter = Reg(UInt(9.W))
    val regDidTransmit = Reg(Bool())

    switch (regState) {
      is (Link.MultiState.Idle) {
        regMaster := false.B
        val isInLow = !io.port.in.si  // Determines whether we will be master
        val startMaster = isInLow && regSiocntStart
        val startSlave = !isInLow && !io.port.in.sc
        when (io.enable && (startMaster || startSlave)) {
          logger.info(cf"Multi begin: master=${isInLow}")

          regDidTransmit := false.B
          rxDataRegs.foreach(r => r := 0xFFFF.U)
          uartTimer.reset := true.B
          regPeerId := 0.U
          regError := false.B
          regTxBuffer := Cat(1.U(1.W), regDataA, 0.U(1.W))
          regPulseCounter := 17.U // (1 start, 16 data, 1 stop) minus 1
          regBusy := true.B

          when (isInLow) {
            regMyId := 0.U
            regMaster := true.B
            regState := Link.MultiState.MasterTransmit
          } .otherwise {
            regState := Link.MultiState.SlaveReceive
          }
        }
      }
      is (Link.MultiState.MasterTransmit) {
        io.port.dir.sd := true.B
        io.port.out.sd := regTxBuffer(0)
        // TODO: should be receiving here too?

        when (io.enable && uartTimer.io.pulse) {
          logger.debug("Master TX pulse")
          regTxBuffer := regTxBuffer >> 1
          regPulseCounter := regPulseCounter - 1.U
          when (regPulseCounter === 0.U) {
            logger.debug("Master TX end")
            regState := Link.MultiState.MasterWait
            regWaitCounter := 0.U
            regDataB0 := regDataA // XXX: should instead be storing what is seen on input pins?
          }
        }
      }
      is (Link.MultiState.MasterWait) {
        io.port.out.so := false.B

        val nextWaitCounter = regWaitCounter + 1.U
        when (io.enable) {
          regWaitCounter := nextWaitCounter

          when (!io.port.in.sd) {
            logger.debug("Master: saw slave start bit")
            regPeerId := regPeerId + 1.U
            regState := Link.MultiState.MasterReceive
            regPulseCounter := 17.U
            uartTimer.reset := true.B
          } .elsewhen (nextWaitCounter === 0.U) {
            logger.debug("Master: wait timed out")
            io.irq := interruptEnable
            regBusy := false.B
            regState := Link.MultiState.Idle
          }
        }
      }
      is (Link.MultiState.MasterReceive) {
        io.port.out.so := false.B

        when (io.enable && uartTimer.io.pulseMid) {
          logger.debug("Master RX mid-pulse")
          regRxBuffer := Cat(io.port.in.sd, regRxBuffer >> 1)
        }
        when (io.enable && uartTimer.io.pulse) {
          logger.debug("Master RX pulse")
          regPulseCounter := regPulseCounter - 1.U
          when (regPulseCounter === 0.U) {
            logger.debug("Master RX end")
            val rxData = regRxBuffer(16, 1)
            rxDataRegs.zipWithIndex.foreach(r => {
              when (regPeerId === r._2.U) {
                r._1 := rxData
              }
            })

            // Check for invalid stop bit
            when (!regRxBuffer(17)) {
              regError := true.B
            }

            when (regPeerId === 3.U) {
              logger.debug("Master: slave 3 ended")
              io.irq := interruptEnable
              regBusy := false.B
              regState := Link.MultiState.Idle
            } .otherwise {
              regState := Link.MultiState.MasterWait
              regWaitCounter := 0.U
            }
          }
        }
      }
      is (Link.MultiState.SlaveReceive) {
        io.port.out.so := !regDidTransmit

        when (io.enable && uartTimer.io.pulseMid) {
          logger.debug("Slave RX mid-pulse")
          regRxBuffer := Cat(io.port.in.sd, regRxBuffer >> 1)
        }
        when (io.enable && uartTimer.io.pulse) {
          logger.debug("Slave RX pulse")
          regPulseCounter := regPulseCounter - 1.U
          when (regPulseCounter === 0.U) {
            logger.debug("Slave RX end")
            val rxData = regRxBuffer(16, 1)
            rxDataRegs.zipWithIndex.foreach(r => {
              when (regPeerId === r._2.U) {
                r._1 := rxData
              }
            })

            // Check for invalid stop bit
            when (!regRxBuffer(17)) {
              regError := true.B
            }

            regState := Link.MultiState.SlaveWait
            regPeerId := regPeerId + 1.U
          }
        }
      }
      is (Link.MultiState.SlaveWait) {
        io.port.out.so := !regDidTransmit

        when (io.enable && !io.port.in.sd) {
          logger.debug("Slave wait: got start bit")
          // Go to Receive
          uartTimer.reset := true.B
          regPulseCounter := 17.U // (1 start, 16 data, 1 stop) minus 1
          regState := Link.MultiState.SlaveReceive
        }
        when (io.enable && !io.port.in.si && !regDidTransmit) {
          logger.debug("Slave wait: saw SI go low")
          // Go to Transmit
          uartTimer.reset := true.B
          regMyId := regPeerId
          regPeerId := regPeerId + 1.U
          regPulseCounter := 17.U // (1 start, 16 data, 1 stop) minus 1
          regState := Link.MultiState.SlaveTransmit
          regDidTransmit := true.B
        }
      }
      is (Link.MultiState.SlaveTransmit) {
        io.port.dir.sd := true.B
        io.port.out.sd := regTxBuffer(0)
        // TODO: should be receiving here too?

        when (io.enable && uartTimer.io.pulse) {
          logger.debug("Slave TX pulse")
          regTxBuffer := regTxBuffer >> 1
          regPulseCounter := regPulseCounter - 1.U
          when (regPulseCounter === 0.U) {
            logger.debug("Slave TX end")
            // XXX: should instead be storing what is seen on input pins?
            rxDataRegs.zipWithIndex.foreach(r => {
              when (regMyId === r._2.U) {
                r._1 := regDataA
              }
            })
            regState := Link.MultiState.SlaveWait
          }
        }
      }
    }

    when (regState =/= Link.MultiState.Idle && !regMaster) {
      when (io.enable && io.port.in.sc) {
        logger.debug("Slave: SC went back high")
        // End
        io.irq := interruptEnable
        regBusy := false.B
        regState := Link.MultiState.Idle
        when (!regDidTransmit) {
          regError := true.B
        }
      }
    }

    when (regState =/= Link.MultiState.Idle && siocntStartUnset) {
      logger.debug("Abort multi")
      regBusy := false.B
      regState := Link.MultiState.Idle
    }
  }

  private def handleJoybus(): Unit = {
    // Whether we should receive bits into the receive buffer (MSB first)
    val doReceive = WireDefault(false.B)
    val doTransmit = WireDefault(false.B)

    val regState = RegInit(Link.JoybusState.Idle)
    // SI high 1024 cycle reset counter
    val regResetCounter = RegInit(0.U(10.W))
    // Current command
    val regCommand = Reg(JoybusCommand.Type())
    // Receive shift register
    val regRxBuffer = Reg(UInt(32.W))
    // Receive bit counter
    val regRxBitCounter = Reg(UInt(6.W))
    // Receive: number of low cycles seen
    val regRxLoCount = Reg(UInt(10.W))
    // Receive: number of high cycles seen
    val regRxHiCount = Reg(UInt(10.W))
    // Transmit shift register
    val regTxBuffer = Reg(UInt(32.W))
    // Transmit bit counter
    val regTxBitCounter = Reg(UInt(6.W))
    // Transmit cycle counter
    val regTxCycleCounter = Reg(UInt(6.W))

    // Handle reseting the state
    val resetState = WireDefault(false.B)
    when (resetState) {
      regState := Link.JoybusState.Idle
    }
    when (io.enable && prevMode =/= Link.Mode.Joybus) {
      resetState := true.B
    }
    // After 1024 cycles of not seeing SI low at all, reset the state back to Idle.
    // This accounts for two behaviors:
    // 1) The GBA, if it's transmitting, and it doesn't see a "SI = 0"
    //    for ~60us, it stops transmitting. This happens naturally because
    //    it normally sees itself transmitting (and producing low values
    //    due to the GCN/GBA adapter).
    // 2) After receiving an unknown command, it takes about ~60us of
    //    no "SI = 0" before it will recognize another command.
    // Any cycle where SI = 0 resets this counter.
    when (io.enable && io.port.in.si && regState =/= Link.JoybusState.Idle) {
      val nextResetCounter = regResetCounter + 1.U
      regResetCounter := nextResetCounter
      when (nextResetCounter === 0.U) {
        logger.info("Joybus: inactivity, resetting")
        resetState := true.B
      }
    }
    when (io.enable && !io.port.in.si) {
      regResetCounter := 0.U
    }

    io.port.dir.so := false.B  // Default: Input (pull-up)
    io.port.dir.si := false.B  // Always Input
    io.port.dir.sd := true.B   // Always output
    io.port.dir.sc := true.B   // Always output
    io.port.out.so := true.B
    io.port.out.si := DontCare
    io.port.out.sd := false.B
    io.port.out.sc := false.B

    switch (regState) {
      is (Link.JoybusState.Idle) {
        when (io.enable && !io.port.in.si) {
          logger.info("Joybus: exiting Idle")
          regState := Link.JoybusState.ReceiveCommand
          regRxBitCounter := 8.U // 8 bit command
          regRxLoCount := 0.U
          regRxHiCount := 0.U
          regResetCounter := 0.U
        }
      }
      is (Link.JoybusState.ReceiveCommand) {
        doReceive := regRxBitCounter > 0.U

        when (io.enable && regRxBitCounter === 0.U) {
          val command = regRxBuffer(7, 0)
          logger.info(cf"Joybus: RX command 0x${command}%x")

          // Set up counters and buffers to receive the payload.
          regState := Link.JoybusState.ReceivePayload
          when (command === 0xFF.U) {
            regCommand := Link.JoybusCommand.Reset
            regRxBitCounter := 0.U
            regTxBitCounter := 16.U
            regTxBuffer := Cat(0x00.U(8.W), 0x04.U(8.W), 0.U(16.W))
          } .elsewhen (command === 0x00.U) {
            regCommand := Link.JoybusCommand.Status
            regRxBitCounter := 0.U
            regTxBitCounter := 16.U
            regTxBuffer := Cat(0x00.U(8.W), 0x04.U(8.W), 0.U(16.W))
          } .elsewhen (command === 0x15.U) {
            regCommand := Link.JoybusCommand.DataWrite
            regRxBitCounter := 32.U
            regTxBitCounter := 0.U
          } .elsewhen (command === 0x14.U) {
            regCommand := Link.JoybusCommand.DataRead
            regRxBitCounter := 0.U
            regTxBitCounter := 32.U
            regTxBuffer := Cat(
              regJoyTrans(7, 0),
              regJoyTrans(15, 8),
              regJoyTrans(23, 16),
              regJoyTrans(31, 24),
            )
          } .otherwise {
           regState := Link.JoybusState.UnknownCommand
          }
        }
      }
      is (Link.JoybusState.ReceivePayload) {
        doReceive := regRxBitCounter > 0.U

        when (io.enable && regRxBitCounter === 0.U) {
          logger.info(cf"Joybus: finished RX payload")
          regState := Link.JoybusState.ReceiveStopBit
        }
      }
      is (Link.JoybusState.ReceiveStopBit) {
        // We enter this state with SI low, so we're waiting for it to go back high.
        when (io.enable && io.port.in.si) {
          logger.info(cf"Joybus stop bit went back high")
          regState := Link.JoybusState.TransmitSetup
          regTxCycleCounter := 0.U
        }
      }
      is (Link.JoybusState.TransmitSetup) {
        io.port.dir.so := true.B
        io.port.out.so := true.B
        when (io.enable) {
          val nextCycleCounter = regTxCycleCounter + 1.U
          regTxCycleCounter := nextCycleCounter
          when (nextCycleCounter === 0.U) {
            regState := Link.JoybusState.TransmitPayload
          }
        }
      }
      is (Link.JoybusState.TransmitPayload) {
        io.port.dir.so := true.B
        doTransmit := regTxBitCounter > 0.U

        when (io.enable && regTxBitCounter === 0.U) {
          logger.info(cf"Joybus: finished TX payload")
          regTxBitCounter := 8.U
          regTxBuffer := Cat(joyStatRead, 0.U(24.W))
          regState := Link.JoybusState.TransmitStatus
        }
      }
      is (Link.JoybusState.TransmitStatus) {
        io.port.dir.so := true.B
        doTransmit := regTxBitCounter > 0.U

        when (io.enable && regTxBitCounter === 0.U) {
          logger.info(cf"Joybus: finished TX status")
          regState := Link.JoybusState.TransmitStopBit
          regTxCycleCounter := 0.U
        }
      }
      is (Link.JoybusState.TransmitStopBit) {
        io.port.dir.so := true.B
        io.port.out.so := false.B
        when (io.enable) {
          val nextCycleCounter = regTxCycleCounter + 1.U
          regTxCycleCounter := nextCycleCounter
          when (nextCycleCounter === 0.U) {
            logger.info(cf"Joybus: Finished stop bit")
            regState := Link.JoybusState.Idle
            io.irq := regJoyCntInterrupt && (regCommand =/= Link.JoybusCommand.Status)

            switch (regCommand) {
              is (Link.JoybusCommand.Reset) {
                regJoyCntReset := true.B
              }
              is (Link.JoybusCommand.DataWrite) {
                // Set regJoyCntReceive when data has been pushed to the GBA.
                regJoyCntReceive := true.B
                regJoyStatRx := true.B
                regJoyRecv := Cat(
                  regRxBuffer(7, 0),
                  regRxBuffer(15, 8),
                  regRxBuffer(23, 16),
                  regRxBuffer(31, 24),
                )
              }
              is (Link.JoybusCommand.DataRead) {
                // Set regJoyCntSend when data has been pulled from GBA.
                regJoyCntSend := true.B
                regJoyStatTx := false.B
              }
            }
          }
        }
      }
    }

    // Receive bit logic
    when (io.enable && doReceive) {
      when (!io.port.in.si) {
        regRxLoCount := regRxLoCount + 1.U
      } .otherwise {
        regRxHiCount := regRxHiCount + 1.U
      }

      // Falling edge: receive a bit
      when (!io.port.in.si && prevPortIn.si) {
        logger.debug(cf"Joybus: RX bit lo=${regRxLoCount} hi=${regRxHiCount}")
        val bit = regRxHiCount > regRxLoCount
        regRxBuffer := Cat(regRxBuffer, bit)
        regRxLoCount := 0.U
        regRxHiCount := 0.U
        regRxBitCounter := regRxBitCounter - 1.U
      }
    }
    // Transmit bit logic
    when (doTransmit) {
      val bit = regTxBuffer(31)
      val counterHi = regTxCycleCounter(5, 4)
      when (counterHi === 0.U) {
        // First quarter: always low
        io.port.out.so := false.B
      } .elsewhen (counterHi === 3.U) {
        // Fourth quarter: always high
        io.port.out.so := true.B
      } .otherwise {
        // Middle two quarters: the bit value
        io.port.out.so := bit
      }

      when (io.enable) {
        val nextCycleCounter = regTxCycleCounter + 1.U
        regTxCycleCounter := nextCycleCounter
        when (nextCycleCounter === 0.U) {
          logger.debug("Joybus: TX bit finished")
          regTxBitCounter := regTxBitCounter - 1.U
          regTxBuffer := regTxBuffer << 1
        }
      }
    }
  }
}

object Link {
  class Ports extends Bundle {
    val so = Bool()
    val si = Bool()
    val sd = Bool()
    val sc = Bool()
  }

  /// Link port: [3=SO, 2=SI, 1=SD, 0=SC]
  class Interface extends Bundle {
    val in = Input(new Ports)
    val out = Output(new Ports)
    val dir = Output(new Ports)
  }

  /// RCNT register: SIO mode / GPIO
  class RegisterRcnt extends Bundle {
    val mode = UInt(2.W)
    val _unused = UInt(5.W)
    /// SI interrupt enable
    val interrupt = Bool()
    val pinDir = UInt(4.W)
    val pinData = UInt(4.W)
  }

  object Mode extends ChiselEnum {
    val Normal = Value
    val Multi = Value
    val Uart = Value
    val Gpio = Value
    val Joybus = Value
  }

  class NormalSiocntLo extends Bundle {
    val busy = Bool()
    val _unused = UInt(3.W)
    val soIdle = Bool()
    val si = Bool()
    val fastClock = Bool()
    val master = Bool()
  }

  /// States for the 'normal' spi state machine
  object NormalState extends ChiselEnum {
    val Idle = Value
    val Ready = Value
    val Active = Value
    val Hold = Value
  }

  class MultiSiocntLo extends Bundle {
    val busy = Bool()
    val error = Bool()
    val id = UInt(2.W)
    val sd = Bool()
    val si = Bool()
    val baud = UInt(2.W)
  }

  /// States for the multiplayer state machine
  object MultiState extends ChiselEnum {
    /// Idle State: waiting for a transfer to begin
    val Idle = Value
    /// Master: Sending out our data
    val MasterTransmit = Value
    /// Master: Waiting for a slave to start sending data
    val MasterWait = Value
    /// Master: Receiving data from a slave
    val MasterReceive = Value
    /// Slave: Receiving data from another peer
    val SlaveReceive = Value
    /// Slave: Waiting for next peer to start
    val SlaveWait = Value
    /// Slave: Sending out our data
    val SlaveTransmit = Value
  }

  /// States for the joybus state machine
  object JoybusState extends ChiselEnum {
    /// Waiting for a command to start
    val Idle = Value
    /// Receiving command byte
    val ReceiveCommand = Value
    /// Receiving command payload (if any)
    val ReceivePayload = Value
    /// Wait for a stop bit following the command+payload
    val ReceiveStopBit = Value
    /// Wait before starting transmit.
    val TransmitSetup = Value
    /// Transmit the response payload (if any)
    val TransmitPayload = Value
    /// Transmit the status byte
    val TransmitStatus = Value
    /// Transmit the final stop bit
    val TransmitStopBit = Value
    /// Ignoring an unknown command
    val UnknownCommand = Value
  }

  object JoybusCommand extends ChiselEnum {
    // 0xFF: Reset
    val Reset = Value
    // 0x00: Status
    val Status = Value
    // 0x15: Data Write (to GBA)
    val DataWrite = Value
    // 0x14: Data Read (from GBA)
    val DataRead = Value
  }
}