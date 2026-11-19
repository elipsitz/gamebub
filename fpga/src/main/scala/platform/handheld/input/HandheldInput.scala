package platform.handheld.input

import chisel3._
import chisel3.util._
import lib.mem.RegisterMap
import lib.util.ButtonFilter
import net.gamebub.framework.CoreException
import net.gamebub.framework.interface.{InputBasicV0, InputExtendedV0}

class HandheldInput private (
  val capabilities: Int,
  val registerEntries: Seq[(Int, RegisterMap.Entry)],
  val handheldButtons: InputBasicV0.Buttons,
  val docked: Bool,
  val coreFocus: Bool,
  val vibrateEnable: Bool,
  val handheldVibrate: UInt,
)

object HandheldInput {
  /** Slot register at +0x00 */
  private class SlotReg0 extends Bundle {
    val deviceType = InputExtendedV0.DeviceType()
    val _padding = UInt(12.W)
    val buttons = new InputExtendedV0.Buttons
  }

  /** Slot register at +0x04 */
  private class SlotReg1 extends Bundle {
    val rightY = SInt(8.W)
    val rightX = SInt(8.W)
    val leftY = SInt(8.W)
    val leftX = SInt(8.W)
  }

  /** Slot register at +0x08 */
  private class SlotReg2 extends Bundle {
    val rightTrigger = UInt(8.W)
    val leftTrigger = UInt(8.W)
  }

  /** Per-slot rumble output */
  private class RumbleSlotOut extends Bundle {
    val weak = UInt(8.W)
    val strong = UInt(8.W)
  }

  def apply(inputInterface: Option[Data], clockSystemHz: Int): HandheldInput = {
    val handheldButtons = Wire(new InputBasicV0.Buttons)
    val docked = Wire(Bool())
    val coreFocus = Wire(Bool())
    val vibrateEnable = Wire(Bool())
    val handheldVibrate = WireDefault(0.U(8.W))

    val (baseCapabilities, slotEntries, rumbleSlots) = inputInterface match {
      case Some(basic: InputBasicV0) => {
        val capabilities = 0x0000_0001
        val regSlot0Buttons = RegInit(0.U.asTypeOf(new InputBasicV0.Buttons))
        val entries = Seq(
          0x1400 -> RegisterMap.Entry(
            32,
            read = RegisterMap.ReadFn(),
            write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
              when (write) {
                val buttons = suppressEnumCastWarning { data.asTypeOf(new SlotReg0) }.buttons
                regSlot0Buttons.a := buttons.a
                regSlot0Buttons.b := buttons.b
                regSlot0Buttons.x := buttons.x
                regSlot0Buttons.y := buttons.y
                regSlot0Buttons.up := buttons.up
                regSlot0Buttons.down := buttons.down
                regSlot0Buttons.left := buttons.left
                regSlot0Buttons.right := buttons.right
                regSlot0Buttons.l := buttons.l1
                regSlot0Buttons.r := buttons.r1
                regSlot0Buttons.start := buttons.start
                regSlot0Buttons.select := buttons.select
              }
            ),
          ),
        )
        val buttonFilter = Module(new ButtonFilter(new InputBasicV0.Buttons))
        buttonFilter.io.enable := coreFocus
        buttonFilter.io.input := (handheldButtons.asUInt | regSlot0Buttons.asUInt).asTypeOf(new InputBasicV0.Buttons)
        basic.buttons := buttonFilter.io.output

        val rumbleSlots = basic.rumble
          .map(r => Seq((Mux(r, 255.U(8.W), 0.U(8.W)), 0.U(8.W))))
          .getOrElse(Seq.empty)
        (capabilities, entries, rumbleSlots)
      }

      case Some(ext: InputExtendedV0) => {
        val n = ext.numSlots
        val capabilities = 0x80 | n
        val regDeviceTypes = RegInit(VecInit(Seq.fill(n)(InputExtendedV0.DeviceType.None)))
        val regButtons = RegInit(VecInit(Seq.fill(n)(0.U.asTypeOf(new InputExtendedV0.Buttons))))
        val regSticks = RegInit(VecInit(Seq.fill(n)(0.U.asTypeOf(new SlotReg1))))
        val regTriggers = RegInit(VecInit(Seq.fill(n)(0.U.asTypeOf(new SlotReg2))))
        val entries = (0 until n).flatMap { i =>
          val base = 0x1400 + i * 0x10
          Seq(
            (base + 0x00) -> RegisterMap.Entry(
              32,
              read = RegisterMap.ReadFn(),
              write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
                when (write) {
                  val reg = suppressEnumCastWarning { data.asTypeOf(new SlotReg0) }
                  regDeviceTypes(i) := reg.deviceType
                  regButtons(i) := reg.buttons
                }
              ),
            ),
            (base + 0x04) -> RegisterMap.Entry.w(regSticks(i)),
            (base + 0x08) -> RegisterMap.Entry.w(regTriggers(i)),
          )
        }

        val extHandheldButtons = WireDefault(0.U.asTypeOf(new InputExtendedV0.Buttons))
        extHandheldButtons.a := handheldButtons.a
        extHandheldButtons.b := handheldButtons.b
        extHandheldButtons.x := handheldButtons.x
        extHandheldButtons.y := handheldButtons.y
        extHandheldButtons.up := handheldButtons.up
        extHandheldButtons.down := handheldButtons.down
        extHandheldButtons.left := handheldButtons.left
        extHandheldButtons.right := handheldButtons.right
        extHandheldButtons.l1 := handheldButtons.l
        extHandheldButtons.r1 := handheldButtons.r
        extHandheldButtons.start := handheldButtons.start
        extHandheldButtons.select := handheldButtons.select

        for (i <- 0 until n) {
          val slot = ext.slots(i)
          val undockedType = if (i == 0) InputExtendedV0.DeviceType.Handheld else InputExtendedV0.DeviceType.None
          val undockedButtons = if (i == 0) extHandheldButtons else 0.U.asTypeOf(new InputExtendedV0.Buttons)

          slot.deviceType := Mux(docked, regDeviceTypes(i), undockedType)
          slot.active := slot.deviceType =/= InputExtendedV0.DeviceType.None

          val buttonFilter = Module(new ButtonFilter(new InputExtendedV0.Buttons))
          buttonFilter.io.enable := coreFocus
          buttonFilter.io.input := Mux(docked, regButtons(i), undockedButtons)
          slot.buttons := buttonFilter.io.output

          val analog = Wire(new InputExtendedV0.Analog)
          analog.leftX := regSticks(i).leftX
          analog.leftY := regSticks(i).leftY
          analog.rightX := regSticks(i).rightX
          analog.rightY := regSticks(i).rightY
          analog.leftTrigger := regTriggers(i).leftTrigger
          analog.rightTrigger := regTriggers(i).rightTrigger
          slot.analog := Mux(docked && coreFocus, analog, 0.U.asTypeOf(new InputExtendedV0.Analog))
        }

        val rumbleSlots = ext.rumble
          .map(_.map(r => (r.strong, r.weak)).toSeq)
          .getOrElse(Seq.empty)
        (capabilities, entries, rumbleSlots)
      }

      case Some(x) => throw new CoreException("Unknown 'input': " + x.getClass())

      case None => (0, Seq.empty, Seq.empty)
    }

    val (rumbleCapabilities, rumbleEntries) = if (rumbleSlots.nonEmpty) {
      // Drive the single handheld motor from slot 0 using the stronger channel
      val rumbleEnabled = coreFocus && vibrateEnable
      val (slot0Strong, slot0Weak) = rumbleSlots.head
      when (rumbleEnabled && !docked) {
        handheldVibrate := Mux(slot0Strong >= slot0Weak, slot0Strong, slot0Weak)
      }

      // Average each slot over 64 samples per 60 Hz window for host polling
      val rumbleWindowHz = 60
      val rumbleSamplesPerWindow = 64
      val samplePeriod = (clockSystemHz / (rumbleWindowHz * rumbleSamplesPerWindow)).max(1)
      val samplePrescaler = Counter(samplePeriod)
      val sampleCounter = Counter(rumbleSamplesPerWindow)
      val sampleTick = WireDefault(false.B)
      val windowEnd = WireDefault(false.B)
      when (!rumbleEnabled) {
        samplePrescaler.reset()
        sampleCounter.reset()
      } .otherwise {
        sampleTick := samplePrescaler.inc()
        when (sampleTick) {
          windowEnd := sampleCounter.inc()
        }
      }

      val accShift = log2Ceil(rumbleSamplesPerWindow)
      val accWidth = 8 + accShift
      val regRumbleOut = RegInit(VecInit(Seq.fill(4)(0.U.asTypeOf(new RumbleSlotOut))))
      for (((strong, weak), i) <- rumbleSlots.zipWithIndex) {
        val regAccStrong = RegInit(0.U(accWidth.W))
        val regAccWeak = RegInit(0.U(accWidth.W))
        when (!rumbleEnabled) {
          regAccStrong := 0.U
          regAccWeak := 0.U
          regRumbleOut(i) := 0.U.asTypeOf(new RumbleSlotOut)
        } .elsewhen (sampleTick) {
          val nextStrong = regAccStrong + strong
          val nextWeak = regAccWeak + weak
          when (windowEnd) {
            // Shift right by log2(64) to compute the 8-bit window average
            regRumbleOut(i).strong := nextStrong(accWidth - 1, accShift)
            regRumbleOut(i).weak := nextWeak(accWidth - 1, accShift)
            regAccStrong := 0.U
            regAccWeak := 0.U
          } .otherwise {
            regAccStrong := nextStrong
            regAccWeak := nextWeak
          }
        }
      }

      // Set rumble capability bit and pack 4 slots into 64 bits (0x1500: slots 0-1, 0x1504: slots 2-3)
      val entries = Seq(
        0x1500 -> RegisterMap.Entry.r(regRumbleOut.asUInt(31, 0)),
        0x1504 -> RegisterMap.Entry.r(regRumbleOut.asUInt(63, 32)),
      )
      (0x40, entries)
    } else {
      (0, Seq.empty)
    }

    new HandheldInput(
      capabilities = baseCapabilities | rumbleCapabilities,
      registerEntries = slotEntries ++ rumbleEntries,
      handheldButtons = handheldButtons,
      docked = docked,
      coreFocus = coreFocus,
      vibrateEnable = vibrateEnable,
      handheldVibrate = handheldVibrate,
    )
  }
}
