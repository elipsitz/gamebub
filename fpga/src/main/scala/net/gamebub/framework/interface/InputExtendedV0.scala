package net.gamebub.framework.interface

import chisel3._

object InputExtendedV0 {
  object DeviceType extends ChiselEnum {
    val None = Value(0.U(4.W))
    val Handheld = Value(1.U)
    val DigitalGamepad = Value(2.U)
    val AnalogGamepad = Value(3.U)
  }

  class Buttons extends Bundle {
    val a = Bool()
    val b = Bool()
    val x = Bool()
    val y = Bool()
    val up = Bool()
    val down = Bool()
    val left = Bool()
    val right = Bool()
    val l1 = Bool()
    val r1 = Bool()
    val l2 = Bool()
    val r2 = Bool()
    val l3 = Bool()
    val r3 = Bool()
    val start = Bool()
    val select = Bool()
  }

  /** Analog sticks (-127 is left/up, +127 is right/down) and triggers (0: released, 255: fully pressed) */
  class Analog extends Bundle {
    val leftX = SInt(8.W)
    val leftY = SInt(8.W)
    val rightX = SInt(8.W)
    val rightY = SInt(8.W)
    val leftTrigger = UInt(8.W)
    val rightTrigger = UInt(8.W)
  }

  class Slot extends Bundle {
    /** High when a device is present in this slot (deviceType =/= None) */
    val active = Bool()
    val deviceType = DeviceType()
    val buttons = new Buttons()
    val analog = new Analog()
  }
}

class InputExtendedV0(val numSlots: Int = 1) extends Bundle {
  require(numSlots >= 1 && numSlots <= 4, "InputExtendedV0.numSlots must be between 1 and 4")
  val slots = Input(Vec(numSlots, new InputExtendedV0.Slot()))
}
