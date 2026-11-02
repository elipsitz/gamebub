package net.gamebub.core.gameboy

import chisel3._
import _root_.circt.stage.ChiselStage
import gameboy.apu.ApuOutput
import gameboy.cart.emu.{EmuCartConfig, EmuCartridge, EmuCartridgeDataAccess, EmuMbc}
import gameboy.{ClockConfig, Gameboy, JoypadState}
import gameboy.ppu.PpuOutput

object SimGameboy extends App {
  ChiselStage.emitSystemVerilogFile(
    new SimGameboy,
    args,
    firtoolOpts = Array(
      "--preserve-aggregate=1d-vec",
      "-enable-layers=Verification",
      "-enable-layers=Verification.Assert",
      "-enable-layers=Verification.Assume",
      "-enable-layers=Verification.Cover",
    )
  )
}

class SimGameboy extends Module {
  val io = IO(new Bundle {
    val clockConfig = new ClockConfig
    /** Run as a Game Boy Color (otherwise original Game Boy) */
    val isCgb = Input(Bool())

    val ppu = new PpuOutput
    val joypad = Input(new JoypadState)
    val apu = new ApuOutput

    // Emulated cartridge data access interface
    val cartConfig = Input(new EmuCartConfig())
    val dataAccess = new EmuCartridgeDataAccess()
  })

  // Gameboy
  val gameboyConfig = Gameboy.Configuration(
    skipBootrom = false,
    optimizeForSimulation = true,
  )
  val gameboy = Module(new Gameboy(gameboyConfig))
  gameboy.io.isCgb := io.isCgb
  io.clockConfig <> gameboy.io.clockConfig
  io.ppu <> gameboy.io.ppu
  io.joypad <> gameboy.io.joypad
  io.apu <> gameboy.io.apu

  // Boot ROM, to be filled in by verilator simulator
  val bootRom = {
    val rom = SyncReadMem(2048 + 256, UInt(8.W))
    // dontTouch: hack to ensure Chisel doesn't optimize the mem out
    val temp = dontTouch(WireDefault(false.B))
    when (temp) {
      rom.write(0.U, 0.U)
    }
    rom
  }
  gameboy.io.bootRom.data := bootRom.read(gameboy.io.bootRom.address, gameboy.io.bootRom.read)

  // Disconnected serial
  gameboy.io.serial.clockIn := true.B
  gameboy.io.serial.in := true.B

  // Emulated cartridge
  val emuCart = Module(new EmuCartridge(4 * 1024 * 1024))
  gameboy.io.cartridge <> emuCart.io.cartridge
  emuCart.io.dataAccess <> io.dataAccess
  emuCart.io.config := io.cartConfig
  emuCart.io.tCycle := gameboy.io.tCycle
  emuCart.io.rtcAccess.writeEnable := false.B
  emuCart.io.rtcAccess.writeState := DontCare
  emuCart.io.rtcAccess.latchSelect := DontCare
  emuCart.io.imu := DontCare
}