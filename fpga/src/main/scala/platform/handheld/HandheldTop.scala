package platform.handheld

import chisel3._
import chisel3.util._
import _root_.circt.stage.ChiselStage
import lib.mem.{MemoryInterface, MemoryMap, RegisterMap}
import lib.util.{FractionalDivider, ButtonFilter}
import lib.video.{Color, ColorARGB, ColorRGB}
import net.gamebub.framework.{Core, CoreException}
import net.gamebub.framework.interface._
import platform.handheld.display._
import platform.handheld.spi.SpiReceiverFifo
import xilinx.{XpmCdcHandshake, XpmCdcSingle, XpmCdcSyncRst}

object HandheldTop extends App {
  // Parse arguments.
  if (args.length < 2) {
    throw new IllegalArgumentException("missing arg 0: core class, arg 1: revision")
  }
  val argCoreClassName :: argRevision :: argRest = args.toList

  // Generate verilog.
  val coreFactory = () =>
    Class
      .forName(argCoreClassName)
      .getDeclaredConstructor()
      .newInstance()
      .asInstanceOf[Core]

  ChiselStage.emitSystemVerilogFile(
    new HandheldTop(coreFactory, getRevision(argRevision)),
    argRest.toArray,
    firtoolOpts = Array(
      "--preserve-aggregate=1d-vec",
    )
  )

  private def getRevision(name: String): Revision = {
    name match {
      case "1" | "2" => Revision(
        displayWidth = 480,
        displayHeight = 320,
        displayRotate = true,
        displayColorDepth = 6,
        displayDriverFactory = (sourceFramePeriod, clockHz) => {
          val driver = Module(new ILI9488(
            clockHz,
            sourceFramePeriod,
          ))
          (driver, driver.io)
        },
        getClockDisplayHz = ILI9488.getClockDisplayHz,
        overlayWidth = 240,
        overlayHeight = 160,
        numSdramChips = 1,
      )
      case "3" => Revision(
        displayWidth = 800,
        displayHeight = 480,
        displayColorDepth = 6,
        displayDriverFactory = (sourceFramePeriod, clockHz) => {
          val driver = Module(new ST7262E43(
            clockHz,
            sourceFramePeriod,
          ))
          (driver, driver.io)
        },
        getClockDisplayHz = (_) => (26_099_000, 26_100_000),
        overlayWidth = 360,
        overlayHeight = 240,
        numSdramChips = 1,
      )
      case "4" => Revision(
        displayWidth = 800,
        displayHeight = 480,
        displayRotate = true,
        displayOffsetX = -28,
        displayColorDepth = 8,
        displayDriverFactory = (sourceFramePeriod, clockHz) => {
          val driver = Module(new ILI9806E(
            clockHz,
            sourceFramePeriod,
          ))
          (driver, driver.io)
        },
        getClockDisplayHz = ILI9806E.getClockDisplayHz,
        overlayWidth = 360,
        overlayHeight = 240,
        numSdramChips = 2,
      )
      case _ => throw new IllegalArgumentException("invalid revision " + name)
    }
  }

  var overlayFullDepth: Boolean = false
}

class HandheldInterrupts extends Bundle {
  val coreRequest = Bool()
  val spiResponseFifoUnderflow = Bool()
  val spiRequestFifoOverflow = Bool()
  val buttonEdge = Bool()
  val coreVblank = Bool()
}

/**
 * Top-level Chisel module for the Handheld.
 */
class HandheldTop[T <: Core](coreFactory: () => T, revision: Revision) extends Module {
  val io = IO(new Bundle {
    /** Clocking **/
    val clockIn50Mhz = Input(Clock())
    val clockOutSys = Output(Clock())
    val clockOutDpi = Output(Clock())
    val clockOutLocked = Output(Bool())

    /** Audio/video clock: DPI when HDMI disabled, 27.027 MHz when HDMI enabled */
    val clock_av = Input(Clock())

    /** MCU interrupt: true to pull it low (active) */
    val mcuIrq = Output(Bool())
    val mcuSpiChipSelect = Input(Bool())
    val mcuSpiClock = Input(Bool())
    val mcuSpiDataIn = Input(UInt(4.W))
    val mcuSpiDataOut = Output(UInt(4.W))
    val mcuSpiDataDir = Output(UInt(4.W))

    val lcd = Output(new DpiSignals)
    val lcdDataR = Output(UInt(revision.displayColorDepth.W))
    val lcdDataG = Output(UInt(revision.displayColorDepth.W))
    val lcdDataB = Output(UInt(revision.displayColorDepth.W))
    val dac = Output(new Bundle {
      val mclk = Output(Bool())
      val wclk = Output(Bool())
      val bclk = Output(Bool())
      val data = Output(UInt(1.W))
    })

    /** HDMI */
    val hdmiEnable = Output(Bool())
    val hdmiClockPowerDown = Output(Bool())
    val hdmiAudioClock = Output(Clock())
    val hdmiAudio = Output(Vec(2, UInt(16.W)))
    val hdmiRgb = Output(UInt(24.W))
    val hdmiCx = Input(UInt(10.W))
    val hdmiCy = Input(UInt(10.W))

    /** Raw button input, not registered or inverted. */
    val buttons = Input(new InputV0.Buttons)

    // Cartridge I/O
    val cartridge3V3Enable = Output(Bool())
    val cartridge5V0Enable = Output(Bool())

    val cartridge = new CartridgePortV0

    val vibrate = Output(Bool())
    val pmod = new PmodV0
    val link = new LinkPortV0

    // SRAM
    val sram = new SramV0(addressWidth = 18, dataWidth = 16)

    // SDRAM
    val sdram = new SdramV0(addressWidth = 13, dataWidth = 16, bankWidth = 2, chips = 1)
  })

  //////////////////////////////////
  // Core
  //////////////////////////////////
  ClocksV0.getClockDisplayHz = revision.getClockDisplayHz
  SdramV0.numChips = revision.numSdramChips
  val core = Module(coreFactory())

  // Clocks
  val (
    clockSpi: Clock,
    clockDisplayHz: Int,
    clockSystemHz: Int,
  ) = core.getInterface("clocks") match {
    case Some(clocks: ClocksV0) => {
      clocks.clockIn50M := io.clockIn50Mhz
      io.clockOutLocked := clocks.locked
      io.clockOutSys := clocks.clockOutSystem
      io.clockOutDpi := clocks.clockOutDisplay

      (
        clocks.clockOutSpi,
        clocks.clockDisplayHz,
        clocks.clockSystemHz,
      )
    }
    case Some(x) => throw new CoreException("Unknown 'clocks': " + x.getClass())
    case None => throw new CoreException("'clocks' is required")
  }

  // Video
  val coreVideo = Wire(new Bundle {
    val dataR = UInt(8.W)
    val dataG = UInt(8.W)
    val dataB = UInt(8.W)
    val dataEnable = Bool()
    val vblank = Bool()
    val hblank = Bool()
  })
  val (
    videoWidth: Int,
    videoHeight: Int,
    videoFramePeriod: Double,
    videoColorDepthR: Int,
    videoColorDepthG: Int,
    videoColorDepthB: Int,
  ) = core.getInterface("video") match {
    case Some(video: VideoV0) => {
      coreVideo.dataR := video.data.r
      coreVideo.dataG := video.data.g
      coreVideo.dataB := video.data.b
      coreVideo.dataEnable := video.dataEnable
      coreVideo.hblank := video.hblank
      coreVideo.vblank := video.vblank
      (
        video.videoWidth,
        video.videoHeight,
        video.framePeriod,
        video.colorDepthR,
        video.colorDepthG,
        video.colorDepthB,
      )
    }
    case Some(x) => throw new CoreException("Unknown 'video': " + x.getClass())
    case None => throw new CoreException("'video' is required")
  }

  // Video filter
  val videoFilterIn = Wire(ColorRGB(videoColorDepthR, videoColorDepthG, videoColorDepthB))
  val videoFilterOut = Wire(ColorRGB(8))
  val videoFilterReset = Wire(Reset())
  val (
    videoFilterLatency: Int,
  ) = core.getInterface("videoFilter")  match {
    case Some(videoFilter: VideoFilterBasicV0) => {
      videoFilter.clock := io.clock_av
      videoFilter.reset := videoFilterReset
      videoFilter.dataIn := videoFilterIn
      videoFilterOut := videoFilter.dataOut
      videoFilter.latency
    }
    case Some(x) => throw new CoreException("Unknown 'videoFilter': " + x.getClass())
    case None => {
      videoFilterOut := videoFilterIn.convertTo(videoFilterOut)
      0
    }
  }

  // Audio
  val coreAudioData = Wire(new Bundle {
    val left = SInt(16.W)
    val right = SInt(16.W)
  })
  core.getInterface("audio") match {
    case Some(audio: AudioV0) => {
      coreAudioData.left := audio.left
      coreAudioData.right := audio.right
    }
    case Some(x) => throw new CoreException("Unknown 'audio': " + x.getClass())
    case None => {
      coreAudioData.left := 0.S
      coreAudioData.right := 0.S
    }
  }

  // Host
  val coreHostInterface = Wire(new MemoryInterface(addressWidth = 32, dataWidth = 32))
  val coreCommandHost = Wire(new HostV0.CommandChannel)
  val coreCommandCore = Wire(new HostV0.CommandChannel)
  core.getInterface("host") match {
    case Some(host: HostV0) => {
      coreHostInterface.unsafe :<>= host.mem.unsafe
      host.commandHost <> coreCommandHost
      host.commandCore <> coreCommandCore
    }
    case Some(x) => throw new CoreException("Unknown 'host': " + x.getClass())
    case None => throw new CoreException("'host' is required")
  }

  // PMOD
  core.getInterface("pmod") match {
    case Some(pmod: PmodV0) => {
      io.pmod <> pmod
    }
    case Some(x) => throw new CoreException("Unknown 'pmod': " + x.getClass())
    case None => {
      io.pmod.dir := 0.U // All inputs
      io.pmod.out := 0.U
    }
  }

  // Input
  val coreInput = Wire(new InputV0.Buttons)
  core.getInterface("input") match {
    case Some(input: InputV0) => {
      input.buttons := coreInput
    }
    case Some(x) => throw new CoreException("Unknown 'input': " + x.getClass())
    case None => {}
  }
  
  // Vibrate
  val coreVibrate = Wire(VibrateV0.Mode())
  core.getInterface("vibrate") match {
    case Some(vibrate: VibrateV0) => {
      coreVibrate := vibrate.mode
    }
    case Some(x) => throw new CoreException("Unknown 'vibrate': " + x.getClass())
    case None => {
      coreVibrate := VibrateV0.Mode.Off
    }
  }

  // Cartridge Port
  core.getInterface("cartridge") match {
    case Some(cartridge: CartridgePortV0) => {
      io.cartridge <> cartridge
    }
    case Some(x) => throw new CoreException("Unknown 'cartridge': " + x.getClass())
    case None => {
      io.cartridge.enabled := false.B
      io.cartridge.bank0Out := DontCare
      io.cartridge.bank1Out := DontCare
      io.cartridge.bank2Out := DontCare
      io.cartridge.bank3Out := DontCare
      io.cartridge.pin30Out := DontCare
      io.cartridge.pin31Out := DontCare
      io.cartridge.bank0Dir := false.B
      io.cartridge.bank1Dir := false.B
      io.cartridge.bank2Dir := false.B
      io.cartridge.bank3Dir := false.B
      io.cartridge.pin30Dir := false.B
      io.cartridge.pin31Dir := false.B
    }
  }

  // Link Port
  core.getInterface("link") match {
    case Some(link: LinkPortV0) => {
      io.link <> link
    }
    case Some(x) => throw new CoreException("Unknown 'link': " + x.getClass())
    case None => {
      io.link.soOut := false.B
      io.link.siOut := false.B
      io.link.sdOut := false.B
      io.link.scOut := false.B
      io.link.soDir := false.B
      io.link.siDir := false.B
      io.link.sdDir := false.B
      io.link.scDir := false.B
    }
  }

  // SRAM
  core.getInterface("sram") match {
    case Some(sram: SramV0) => {
      io.sram <> sram
    }
    case Some(x) => throw new CoreException("Unknown 'sram': " + x.getClass())
    case None => {
      io.sram.ceN := true.B
      io.sram.weN := true.B
      io.sram.oeN := true.B
      io.sram.writeMaskN := true.B
      io.sram.address := DontCare
      io.sram.dataOut := DontCare
      io.sram.dataDir := false.B
    }
  }

  // SDRAM
  core.getInterface("sdram") match {
    case Some(sdram: SdramV0) => {
      assert(sdram.chips <= revision.numSdramChips)
      io.sdram <> sdram
    }
    case Some(x) => throw new CoreException("Unknown 'sdram': " + x.getClass())
    case None => {
      io.sdram.clock := false.B.asClock
      io.sdram.cke := false.B
      io.sdram.cs := true.B
      io.sdram.ras := true.B
      io.sdram.cas := true.B
      io.sdram.we := true.B
      io.sdram.dqm := DontCare
      io.sdram.bank := DontCare
      io.sdram.address := DontCare
      io.sdram.dataOut := DontCare
      io.sdram.dataDir := false.B
    }
  }

  //////////////////////////////////
  // MCU Communication
  //////////////////////////////////
  // D0: PICO, D1: POCI
  // TODO: clock gate when nCS is high
  val clockSpiLocked = Wire(Bool())
  val spi = Module(new SpiReceiverFifo())
  spi.io.clockSpi := clockSpi
  spi.io.clockSpiLocked := clockSpiLocked
  io.mcuSpiDataDir := Mux(io.mcuSpiChipSelect, 0.U, spi.io.signals.serialDir)
  io.mcuSpiDataOut := spi.io.signals.serialOut
  spi.io.signals.serialClock := io.mcuSpiClock
  spi.io.signals.serialIn := io.mcuSpiDataIn
  spi.io.signals.chipSelect := io.mcuSpiChipSelect
  withClock (clockSpi) {
    clockSpiLocked := RegNext(!spi.io.clockSpiPowerDown)
  }

  val controlCoreFocus = RegInit(false.B)
  val controlVibrate = RegInit(0.U.asTypeOf(new Bundle() {
    /** True to enable vibration (if the core uses it) */
    val enable = Bool()
  }))
  val controlInterruptEnable = RegInit(0.U.asTypeOf(new HandheldInterrupts))
  val controlInterruptPending = RegInit(0.U.asTypeOf(new HandheldInterrupts))
  val controlButtonForce = RegInit(0.U.asTypeOf(new InputV0.Buttons))
  val controlDock = RegInit(0.U.asTypeOf(new Bundle() {
    /** True if the device is docked */
    val docked = Bool()
  }))
  val controlVideoFilter = RegInit(0.U.asTypeOf(new Bundle {
    val darken = Bool()
  }))

  val controlCommandHost = RegInit(0.U.asTypeOf(Output(new HostV0.CommandChannel)))
  val controlCommandCore = RegInit(0.U.asTypeOf(Output(new HostV0.CommandChannel)))

  val controlButtonRemap = RegInit(VecInit(
    Seq.tabulate((new InputV0.Buttons).getWidth)(i =>
      i.U(log2Ceil(new(InputV0.Buttons).getWidth).W)
    )
  ))

  /// Synchronized physical button state (without MCU force override)
  val buttonState = Wire(new InputV0.Buttons)

  val registerMap = RegisterMap(
    addressWidth = 16,
    dataWidth = 32,
    entries = Seq(
      // Read-only informational registers
      // Framework version
      0x0000 -> RegisterMap.Entry.r("hB0000002".U),
      // System clock frequency (Hz)
      0x0004 -> RegisterMap.Entry.r(clockSystemHz.U),
      // Video dimensions
      0x0100 -> RegisterMap.Entry.r(Cat(videoWidth.U(16.W), videoHeight.U(16.W))),
      // Video color depth
      0x0104 -> RegisterMap.Entry.r(Cat(videoColorDepthR.U(8.W), videoColorDepthG.U(8.W), videoColorDepthB.U(8.W))),

      // Framework control
      0x1000 -> RegisterMap.Entry.rw(controlInterruptEnable),
      0x1004 -> RegisterMap.Entry(
        controlInterruptPending.getWidth,
        read = RegisterMap.ReadFn((_: Bool) => controlInterruptPending.asUInt),
        write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
          when (write) {
            // Write set bits to ack interrupts.
            controlInterruptPending := (controlInterruptPending.asUInt & (~data).asUInt).asTypeOf(controlInterruptPending)
          }
        ),
      ),
      0x1008 -> RegisterMap.Entry.w(controlButtonForce),
      0x100C -> RegisterMap.Entry.w(controlDock),
      0x1010 -> RegisterMap.Entry.w(controlCoreFocus),
      0x1014 -> RegisterMap.Entry.w(controlVibrate),
      0x1018 -> RegisterMap.Entry.w(controlVideoFilter),

      0x1100 -> RegisterMap.Entry.rw(controlCommandHost),
      0x1104 -> RegisterMap.Entry.rw(controlCommandCore),

      // 0x1200 to 0x12FF: button remap

      // Framework status
      0x2000 -> RegisterMap.Entry.r(buttonState),
      0x2004 -> RegisterMap.Entry.r(RegNext(RegNext(io.cartridge.switch))),
    )
    ++ Seq.tabulate(controlButtonRemap.length / 4)(i =>
      (0x1200 + i * 4) -> RegisterMap.Entry(
        32,
        read = RegisterMap.ReadFn((_: Bool) => 0.U),
        write = RegisterMap.WriteFn((write: Bool, data: UInt) =>
          when (write) {
            controlButtonRemap(i * 4 + 0) := data(7, 0)
            controlButtonRemap(i * 4 + 1) := data(15, 8)
            controlButtonRemap(i * 4 + 2) := data(23, 16)
            controlButtonRemap(i * 4 + 3) := data(31, 24)
          }
        ),
      ),
    )
  )

  val overlayInterface = Wire(new MemoryInterface(addressWidth = 18, dataWidth = 16))
  val framebufferInterface = Wire(new MemoryInterface(addressWidth = 18, dataWidth = 16))
  // 16 bit prefix: 64 KiB
  // 12 bit prefix: 1 MiB
  // 8 bit prefix: 16 MiB
  // 4 bit prefix: 256 MiB
  spi.io.mem <> MemoryMap(
    addressWidth = 32,
    dataWidth = 32,
    entries = Seq(
      // Reserve 0xFxxx_xxxx and up for framework
      0xF1.U(8.W) -> registerMap,
      0xF2.U(8.W) -> overlayInterface,
      0xF3.U(8.W) -> framebufferInterface,
    ),
    default = Some(coreHostInterface),
  )

  when (spi.io.debugRequestOverflow) {
    controlInterruptPending.spiRequestFifoOverflow := true.B
  }
  when (spi.io.debugResponseUnderflow) {
    controlInterruptPending.spiResponseFifoUnderflow := true.B
  }

  //////////////////////////////////
  // Interrupts
  //////////////////////////////////
  io.mcuIrq := (controlInterruptPending.asUInt & controlInterruptEnable.asUInt).orR
  when (coreVideo.vblank && !RegNext(coreVideo.vblank)) {
    controlInterruptPending.coreVblank := true.B
  }
  when (coreCommandCore.request && !RegNext(coreCommandCore.request)) {
    controlInterruptPending.coreRequest := true.B
  }

  //////////////////////////////////
  // Input & Vibrate
  //////////////////////////////////
  {
    // Invert and synchronize buttons
    val regButtons = RegNext(RegNext(~io.buttons.asUInt)).asTypeOf(new InputV0.Buttons)
    buttonState := regButtons

    when (regButtons.asUInt =/= RegNext(regButtons.asUInt)) {
      // Button edge, mark interrupt
      controlInterruptPending.buttonEdge := true.B
    }
  }

  // Apply remap registers
  val remappedButtons = {
    val n = controlButtonRemap.length
    val raw = VecInit(buttonState.asUInt.asBools :+ false.B)
    val remappedVec = VecInit(Seq.tabulate(n) { i =>
      val source = controlButtonRemap(i)
      raw(Mux(source < n.U, source, n.U))
    })
    remappedVec.asUInt
  }

  // Only pass input through when the core is focused
  val buttonFilter = Module(new ButtonFilter(new InputV0.Buttons))
  buttonFilter.io.enable := controlCoreFocus
  buttonFilter.io.input := (remappedButtons | controlButtonForce.asUInt).asTypeOf(new InputV0.Buttons)
  coreInput := buttonFilter.io.output

  val vibrateEnabled = controlCoreFocus && controlVibrate.enable && !controlDock.docked
  io.vibrate := RegNext(coreVibrate === VibrateV0.Mode.On && vibrateEnabled)

  //////////////////////////////////
  // Video
  //////////////////////////////////
  io.hdmiEnable := controlDock.docked

  // Double buffering
  val framebuffers = (0 until 2).map(_ =>
    SRAM(
      videoWidth * videoHeight, UInt((videoColorDepthR + videoColorDepthG + videoColorDepthB).W),
      readPortClocks = Seq(io.clock_av), writePortClocks = Seq(), readwritePortClocks = Seq(clock)
    )
  )
  /// Last completed frame
  val regLastFrameComplete = RegInit(0.U(1.W))

  val overlayWidth = revision.overlayWidth
  val overlayHeight = revision.overlayHeight
  val overlayBits = if (HandheldTop.overlayFullDepth) { 16 } else { 2 }
  val overlayFramebuffer = SRAM(
    overlayWidth * overlayHeight, UInt(overlayBits.W),
    readPortClocks = Seq(io.clock_av), writePortClocks = Seq(clock), readwritePortClocks = Seq(),
  )

  // Keep HDMI MMCM powered for a few more cycles after switching away
  // from it to ensure the clock mux functions correctly.
  val hdmiClockPowerTimer = RegInit(0.U(3.W))
  when (controlDock.docked) {
    hdmiClockPowerTimer := 7.U
  } .elsewhen (hdmiClockPowerTimer > 0.U) {
    hdmiClockPowerTimer := hdmiClockPowerTimer - 1.U
  }
  io.hdmiClockPowerDown := hdmiClockPowerTimer === 0.U

  val reset_av = withClock(io.clock_av) { XpmCdcSyncRst(reset) }
  withClockAndReset (clock = io.clock_av, reset = reset_av) {
    val videoX = Wire(UInt(10.W))
    val videoY = Wire(UInt(10.W))
    val framebufferReadAddress = Wire(UInt(log2Ceil(videoWidth * videoHeight).W))
    val overlayReadAddress = Wire(UInt(log2Ceil(overlayWidth * overlayHeight).W))

    val audioData = XpmCdcHandshake.continuous(clock, coreAudioData)

    // Buffering the read allows this to be a block ram instead of distributed ram
    // and an additional output buffer allows Vivado to improve timing.
    //
    // Read from the correct framebuffer.
    val framebufferIndex = Wire(UInt(1.W))
    val lastFrameComplete = XpmCdcSingle(clock, regLastFrameComplete.asBool).asUInt
    for (i <- 0 until 2) {
      framebuffers(i).readPorts(0).enable := framebufferIndex === i.U
      framebuffers(i).readPorts(0).address := framebufferReadAddress
    }
    val framebufferRead = MuxLookup(framebufferIndex, 0.U)(
      (0 until 2).map(i => i.U -> RegNext(RegNext(framebuffers(i).readPorts(0).data)))
    ).asTypeOf(ColorRGB(videoColorDepthR, videoColorDepthG, videoColorDepthB))

    // Apply core video filter
    videoFilterIn := framebufferRead
    val framebufferColor = videoFilterOut
    videoFilterReset := reset_av

    // Similar for overlay framebuffer.
    overlayFramebuffer.readPorts(0).enable := true.B
    overlayFramebuffer.readPorts(0).address := overlayReadAddress
    val overlayReadRaw = RegNext(RegNext(overlayFramebuffer.readPorts(0).data))
    val overlayRead = if (HandheldTop.overlayFullDepth) {
      overlayReadRaw.asTypeOf(ColorARGB(1, 5, 5, 5)).convertTo(ColorARGB(1, 8, 8, 8))
    } else {
      val lum = VecInit(DontCare, 0x0.U, 0x80.U, 0xFF.U)(overlayReadRaw)
      val color = Wire(ColorARGB(1, 8, 8, 8))
      color.a := overlayReadRaw =/= 0.U
      color.r := lum
      color.g := lum
      color.b := lum
      color
    }

    val framebufferInBounds = Wire(Bool())
    val overlayInBounds = Wire(Bool())
    val videoOutput = ColorRGB(8, 8, 8).make(r = 0, g = 0, b = 0)
    val darkenVideo = XpmCdcSingle(clock, controlVideoFilter.darken)
    when (framebufferInBounds) {
      val color = framebufferColor.convertTo(videoOutput)
      when (darkenVideo) {
        videoOutput.r := color.r >> 1
        videoOutput.g := color.g >> 1
        videoOutput.b := color.b >> 1
      } .otherwise {
        videoOutput := color
      }
    }
    when (overlayRead.a.asBool && overlayInBounds) {
      videoOutput := overlayRead.convertTo(videoOutput)
    }

    // DPI video signal output
    val (dpiDriver, dpiDriverIo) = revision.displayDriverFactory(
      /* sourceFramePeriod = */ videoFramePeriod,
      /* clockHz = */ clockDisplayHz,
    )
    dpiDriverIo.lastRenderedFrame := lastFrameComplete
    io.lcd := dpiDriverIo.signals
    val lcdData = videoOutput.convertTo(
      ColorRGB(
        revision.displayColorDepth,
        revision.displayColorDepth,
        revision.displayColorDepth,
      ))
    io.lcdDataR := lcdData.r
    io.lcdDataG := lcdData.g
    io.lcdDataB := lcdData.b

    /**
     * HDMI audio and video signal output
     * Video ID Code 2: 720x480 @ 60Hz
     */
    val hdmiFrameWidth = 858
    val hdmiFrameHeight = 525
    io.hdmiAudio := VecInit(audioData.left.asUInt, audioData.right.asUInt)
    io.hdmiAudioClock := DontCare
    // Pad to 24-bit RGB.
    io.hdmiRgb := videoOutput.convertTo(ColorRGB(8, 8, 8)).asUInt
    val regHdmiFrame = RegInit(0.U(1.W))

    val hdmiEnable = XpmCdcSingle(clock, controlDock.docked)
    when (hdmiEnable) {
      dpiDriver.reset := true.B
      val screenWidth = 720
      val screenHeight = 480

      // Correct HDMI video X and Y
      videoX := io.hdmiCx
      videoY := io.hdmiCy
      when (io.hdmiCx >= screenWidth.U) {
        // Make it so that adding wraps around to 0.
        // (frameWidth - 1) should be (2**width - 1)
        videoX := io.hdmiCx + ((1 << io.hdmiCx.getWidth) - hdmiFrameWidth).U
        videoY := io.hdmiCy + 1.U
        when (io.hdmiCy === (hdmiFrameHeight - 1).U) {
          videoY := 0.U
        }
      }
      val hdmiFramePulse = io.hdmiCy === (hdmiFrameHeight - 1).U
      framebufferIndex := regHdmiFrame
      when (hdmiFramePulse && !RegNext(hdmiFramePulse)) {
        regHdmiFrame := lastFrameComplete
      }

      // Scale and center framebuffer within output video.
      val videoScale = (screenWidth / videoWidth).min(screenHeight / videoHeight)
      val videoOffsetX = (screenWidth - (videoWidth * videoScale)) / 2
      val videoOffsetY = (screenHeight - (videoHeight * videoScale)) / 2
      val framebufferReadDelay = 3 /* reading */ + videoFilterLatency
      framebufferReadAddress :=
        (((videoY - videoOffsetY.U) / videoScale.U) * videoWidth.U) +
          ((videoX - videoOffsetX.U + framebufferReadDelay.U) / videoScale.U)
      framebufferInBounds := videoX >= videoOffsetX.U &&
        videoX < (videoOffsetX + (videoWidth * videoScale)).U &&
        videoY >= videoOffsetY.U &&
        videoY < (videoOffsetY + (videoHeight * videoScale)).U

      // Scale overlay
      val overlayScale = (screenWidth / overlayWidth).min(screenHeight / overlayHeight)
      val overlayOffsetX = (screenWidth - (overlayWidth * overlayScale)) / 2
      val overlayOffsetY = (screenHeight - (overlayHeight * overlayScale)) / 2
      val overlayReadDelay = 3
      overlayReadAddress :=
        (((videoY - overlayOffsetY.U) / overlayScale.U)(8, 0) * overlayWidth.U) +
          ((videoX - overlayOffsetX.U + overlayReadDelay.U) / overlayScale.U)(8, 0)
      overlayInBounds :=
        videoX >= overlayOffsetX.U &&
          videoX < (overlayOffsetX + (overlayWidth * overlayScale)).U &&
          videoY >= overlayOffsetY.U &&
          videoY < (overlayOffsetY + (overlayHeight * overlayScale)).U

      // HDMI Audio
      val audioClock = RegInit(false.B)
      val audioCounter = Counter(27027000 / (48000 * 2))
      when (audioCounter.inc()) {
        audioClock := !audioClock
      }
      io.hdmiAudioClock := audioClock.asClock
    } .otherwise {
      val screenWidth = revision.displayWidth
      val screenHeight = revision.displayHeight

      val dpiX = if (revision.displayRotate) dpiDriverIo.pixelY else dpiDriverIo.pixelX
      val dpiY = if (revision.displayRotate) dpiDriverIo.pixelX else dpiDriverIo.pixelY
      videoX := dpiX
      videoY := dpiY
      framebufferIndex := dpiDriverIo.displayFrame

      // Scale and center framebuffer without output video.
      val videoScale = (screenWidth / videoWidth).min(screenHeight / videoHeight)
      val videoOffsetX = (screenWidth - (videoWidth * videoScale)) / 2 + revision.displayOffsetX
      val videoOffsetY = (screenHeight - (videoHeight * videoScale)) / 2
      val framebufferReadDelay = 3 /* reading */ + videoFilterLatency
      val framebufferReadDelayX = if (revision.displayRotate) 0 else framebufferReadDelay
      val framebufferReadDelayY = if (revision.displayRotate) framebufferReadDelay else 0
      framebufferReadAddress :=
        (((dpiY - videoOffsetY.U + framebufferReadDelayY.U) / videoScale.U) * videoWidth.U) +
          ((dpiX - videoOffsetX.U + framebufferReadDelayX.U) / videoScale.U)
      framebufferInBounds :=
        dpiX >= videoOffsetX.U &&
        dpiX < (videoOffsetX + (videoWidth * videoScale)).U &&
        dpiY >= videoOffsetY.U &&
        dpiY < (videoOffsetY + (videoHeight * videoScale)).U

      // Scale overlay
      val overlayScale = (screenWidth / overlayWidth).min(screenHeight / overlayHeight)
      val overlayOffsetX = (screenWidth - (overlayWidth * overlayScale)) / 2 + revision.displayOffsetX
      val overlayOffsetY = (screenHeight - (overlayHeight * overlayScale)) / 2
      val overlayReadDelay = 3
      val overlayReadDelayX = if (revision.displayRotate) 0 else overlayReadDelay
      val overlayReadDelayY = if (revision.displayRotate) overlayReadDelay else 0
      overlayReadAddress :=
        (((dpiY - overlayOffsetY.U + overlayReadDelayY.U) / overlayScale.U)(8, 0) * overlayWidth.U) +
          ((dpiX - overlayOffsetX.U + overlayReadDelayX.U) / overlayScale.U)(8, 0)
      overlayInBounds :=
        dpiX >= overlayOffsetX.U &&
        dpiX < (overlayOffsetX + (overlayWidth * overlayScale)).U &&
        dpiY >= overlayOffsetY.U &&
        dpiY < (overlayOffsetY + (overlayHeight * overlayScale)).U
      // TODO: re-add overlay X/Y positioning control if needed
    }
  }

  //////////////////////////////////
  // Audio
  //////////////////////////////////
  val reset50M = withClock(io.clockIn50Mhz) { XpmCdcSyncRst(reset) }
  withClockAndReset (clock = io.clockIn50Mhz, reset = reset50M) {
    // Synchronize audio data into this domain
    val syncAudioData = XpmCdcHandshake.continuous(clock, coreAudioData)

    // 16-bit, 2 channel audio output at 48 kHz
    // MCLK = 48 KHz * 256 = 12.288 MHz
    val mclkFactor = 256
    val bitWidth = 16
    val channels = 2
    val regMClock = Reg(Bool())
    val divider = Module(new FractionalDivider(inputHz = 50_000_000, targetHz = 12_288_000 * 2))
    when (divider.io.pulse) {
      regMClock := !regMClock
    }
    val mclkEdge = divider.io.pulse && !regMClock

    val regSample = RegInit(0.U((bitWidth * channels).W))
    val regWordClock = RegInit(false.B)
    val regBitClock = RegInit(true.B)

    val bitClockCounter = Counter(mclkFactor / bitWidth / channels / 2)
    val sampleCounter = Counter(mclkFactor)

    when (mclkEdge) {
      when (bitClockCounter.inc()) {
        regBitClock := !regBitClock
        when (!regBitClock) {
          // Rising edge of bit clock
          regWordClock := false.B
          regSample := regSample << 1
        }
      }
      when (sampleCounter.inc()) {
        regSample := syncAudioData.asUInt
        regWordClock := true.B
      }
    }

    io.dac.mclk := regMClock
    io.dac.wclk := regWordClock
    io.dac.bclk := regBitClock
    io.dac.data := regSample(regSample.getWidth - 1)
  }

  // Overlay (host UI) access.
  overlayInterface.dataRead := DontCare
  overlayInterface.done := false.B
  overlayFramebuffer.writePorts(0).enable := overlayInterface.enable && overlayInterface.write
  overlayFramebuffer.writePorts(0).address := (overlayInterface.address >> 1).asUInt
  val overlayWriteData = overlayInterface.dataWrite.asTypeOf(ColorARGB.argb1555())
  if (HandheldTop.overlayFullDepth) {
    // Full depth color (16 bit), no conversion
    overlayFramebuffer.writePorts(0).data := overlayWriteData.asUInt
  } else {
    // Downconvert to reduced 2-bit palette:
    // [transparent, black, gray, white]
    val color = WireDefault(0.U(2.W))
    when (overlayWriteData.a.asBool) {
      color := VecInit(1.U, 2.U, 2.U, 3.U)(overlayWriteData.r(4, 3))
    }
    overlayFramebuffer.writePorts(0).data := color
  }
  overlayInterface.done := RegNext(overlayInterface.enable)

  // Framebuffer read via SPI.
  for (i <- 0 until 2) {
    framebuffers(i).readwritePorts(0).enable := false.B
    framebuffers(i).readwritePorts(0).address := DontCare
    framebuffers(i).readwritePorts(0).isWrite := DontCare
    framebuffers(i).readwritePorts(0).writeData := DontCare
  }
  val framebufferInterfaceRead = framebufferInterface.enable && !framebufferInterface.write
  when (framebufferInterfaceRead) {
    for (i <- 0 until 2) {
      when (regLastFrameComplete === i.U) {
        framebuffers(i).readwritePorts(0).enable := true.B
        framebuffers(i).readwritePorts(0).address := (framebufferInterface.address >> 1.U).asUInt
        framebuffers(i).readwritePorts(0).isWrite := false.B
      }
    }
  }
  framebufferInterface.dataRead := MuxLookup(regLastFrameComplete, 0.U)(
    (0 until 2).map(i => i.U ->
      RegNext(RegNext(framebuffers(i).readwritePorts(0).readData))
    ))
  framebufferInterface.done := RegNext(RegNext(framebufferInterface.enable))

  //////////////////////////////////
  // Core Connections
  //////////////////////////////////

  // Framebuffer writes
  {
    val framebufferX = RegInit(0.U(log2Ceil(videoWidth).W))
    val framebufferY = RegInit(0.U(log2Ceil(videoHeight).W))
    val framebufferWriteIndex = RegInit(0.U(1.W))

    when (coreVideo.dataEnable && !framebufferInterfaceRead) {
      // Core framebuffer write and SPI framebuffer read share the same read/write port,
      // so ensure that they're not activated at the same time (so they can be inferred correctly).
      val address = (framebufferY * videoWidth.U(10.W)) + framebufferX
      val data = Wire(ColorRGB(videoColorDepthR, videoColorDepthG, videoColorDepthB))
      data.r := coreVideo.dataR
      data.g := coreVideo.dataG
      data.b := coreVideo.dataB
      for (i <- 0 until 2) {
        framebuffers(i).readwritePorts(0).enable := (i.U === framebufferWriteIndex)
        framebuffers(i).readwritePorts(0).address := address
        framebuffers(i).readwritePorts(0).isWrite := true.B
        framebuffers(i).readwritePorts(0).writeData := data.asUInt
      }
    }

    val vblankEdge = coreVideo.vblank && !RegNext(coreVideo.vblank)
    val hblankEdge = coreVideo.hblank && !RegNext(coreVideo.hblank)
    when (vblankEdge) {
      regLastFrameComplete := framebufferWriteIndex
      framebufferWriteIndex := !framebufferWriteIndex
    }

    when (coreVideo.vblank) {
      // Frame ended
      framebufferX := 0.U
      framebufferY := 0.U
    } .elsewhen (coreVideo.hblank) {
      // Line ended
      when (hblankEdge) {
        framebufferX := 0.U
        framebufferY := framebufferY + 1.U
      }
    } .elsewhen (coreVideo.dataEnable) {
      framebufferX := framebufferX + 1.U
    }
  }

  // Cartridge voltage control: Rev1 and Rev2 only
  io.cartridge3V3Enable := RegNext(io.cartridge.enabled && !io.cartridge.switch)
  io.cartridge5V0Enable := RegNext(io.cartridge.enabled && io.cartridge.switch)

  // Command interface
  coreCommandHost.request := controlCommandHost.request
  controlCommandHost.busy := coreCommandHost.busy
  controlCommandHost.done := coreCommandHost.done
  controlCommandHost.error := coreCommandHost.error
  controlCommandCore.request := coreCommandCore.request
  coreCommandCore.busy := controlCommandCore.busy
  coreCommandCore.done := controlCommandCore.done
  coreCommandCore.error := controlCommandCore.error
}

case class Revision(
  displayWidth: Int,
  displayHeight: Int,
  displayRotate: Boolean = false,
  displayOffsetX: Int = 0,
  displayColorDepth: Int,
  displayDriverFactory: (Double, Int) => (Module, DisplayDriverIO),
  /// A function that returns the clockDisplay clock min Hz and max Hz by frame period
  getClockDisplayHz: (Double) => (Int, Int),
  overlayWidth: Int,
  overlayHeight: Int,
  numSdramChips: Int,
)
