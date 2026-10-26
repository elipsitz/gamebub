package net.gamebub.core.gba

import chisel3.simulator.{ChiselSim, Randomization, Settings}
import chisel3.testing.HasTestingDirectory
import lib.util.EphemeralSimulator._
import org.scalatest.funsuite.AnyFunSuite


class MiniCacheSpec extends AnyFunSuite {
  test("first read at address zero") {
    implicit val temporary: HasTestingDirectory = HasTestingDirectory.temporary(deleteOnExit = true)
    val settings = Settings.defaultRaw[HandheldGba.MiniCache].copy(
      randomization = Randomization.random.copy(randomValue = Some("32'h0"))
    )
    new ChiselSim {}.simulateRaw(new HandheldGba.MiniCache(25, 32), settings = settings) { dut =>
      dut.io.in.enable.poke(false)
      dut.io.out.ready.poke(true)
      dut.reset.poke(true)
      dut.clock.step()
      dut.reset.poke(false)

      dut.io.in.address.poke(0)
      dut.io.in.isWrite.poke(false)
      dut.io.in.enable.poke(true)
      assert(dut.io.out.enable.peek().litToBoolean)
      dut.clock.step()
      dut.io.in.enable.poke(false)
      dut.io.out.dataRead.poke(0xEA00002EL)
      assert(dut.io.in.ready.peek().litToBoolean)
      assert(dut.io.in.dataRead.peek().litValue == 0xEA00002EL)
    }
  }
}
