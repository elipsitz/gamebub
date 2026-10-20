package platform.handheld.display

import chisel3._
import chisel3.util._
import platform.handheld.display.DisplayDriverIO

object ILI9806E {
    def getClockDisplayHz(framePeriod: Double): (Int, Int) = {
        // TODO: this is for 60 Hz, calculate dynamically
        (25_800_000, 35_000_000)
    }
}

/** Display Driver for the ILI9806E */
class ILI9806E(
    /** Display clock (Hz) */
    clockHz: Int,
    /** Typical source frame period (seconds)  */
    sourceFramePeriod: Double,
) extends Module {
    val hActive = 480
    val vActive = 800

    val io = IO(new DisplayDriverIO(hActive, vActive))

    val currentFrame = RegInit(0.U(1.W))
    /// Whether the display is currently synchronized with the render
    val regLocked = RegInit(true.B)
    /// Whether we got a new frame, delaying vblank for the next one to resync
    val regResyncReady = RegInit(false.B)
    val regResyncWait = RegInit(false.B)
    val newFrameReady = io.lastRenderedFrame =/= currentFrame
    io.displayFrame := currentFrame

    val regHsync = RegInit(true.B)
    val regVsync = RegInit(true.B)
    val regActive = RegInit(false.B)
    io.signals.dotclk := clock
    io.signals.hsync := regHsync
    io.signals.vsync := regVsync
    io.signals.enable := regActive

    // Although the datasheet says 1, 1, 1 is okay, these appear to be the
    // lowest values that actually produce an image without jitter.
    val vSync = 4
    val vBackPorch = 2
    val vFrontPorchMin = 2
    val vFrontPorchMax = 255
    val totalHeightMin = vActive + vSync + vBackPorch + vFrontPorchMin
    val totalHeightMax = vActive + vSync + vBackPorch + vFrontPorchMax

    // ILI9806E: the total h Inactive must be >= 2 microseconds
    val totalWidthMin = hActive + (clockHz.toFloat / 1000000.0 * 2).ceil.toInt

    // At 60fps:
    // Minimum clock: 25.8 MHz
    // Maximum clock: 35.0 MHz
    assert(totalWidthMin * totalHeightMin / sourceFramePeriod < clockHz)
    assert(totalWidthMin * totalHeightMax / sourceFramePeriod > clockHz)

    // If the real clocks per frame is too high, the frame rate will be too low
    // It's easy to extend clocks per frame by extending vertical period
    // So make target clocks per frame slightly *lower* than what we need
    // (by taking the floor)
    val totalWidth = ((sourceFramePeriod * clockHz) / totalHeightMin).floor.toInt

    val hSync = 4
    val hFrontPorchMin = 2
    val hBackPorchMin = 2  // should be 10+ though
    val hBackPorchMax = 126

    var hFrontPorch = hFrontPorchMin
    var hBackPorch = (totalWidth - hActive - hSync - hFrontPorchMin)
    if (hBackPorch > hBackPorchMax) {
        val amount = hBackPorchMax - hBackPorch
        hBackPorch -= amount
        hFrontPorch += amount
    }
    assert(totalWidth >= totalWidthMin)

    val x = RegInit(0.U(log2Ceil(totalWidth).W))
    val y = RegInit(0.U((log2Ceil(totalHeightMax) + 1).W))
    io.pixelX := x - (hSync + hBackPorch).U
    io.pixelY := y - (vSync + vBackPorch).U

    when (x === (totalWidth - 1).U) {
        // Scanline is done
        regHsync := true.B
        x := 0.U
        y := y + 1.U

        val startFrame = WireDefault(false.B)
        when (startFrame) {
            regVsync := true.B
            y := 0.U
            currentFrame := io.lastRenderedFrame
        }

        when (y === (vSync - 1).U) {
            regVsync := false.B
        } .elsewhen ((y >= (totalHeightMin - 1).U) && newFrameReady) {
            when (regResyncReady) {
                // Skip this frame.
                regResyncReady := false.B
                regResyncWait := true.B
                currentFrame := io.lastRenderedFrame
            } .otherwise {
                // New frame available, start rendering.
                startFrame := true.B
                regResyncWait := false.B
                regLocked := true.B
            }
        } .elsewhen (regResyncWait) {
            // We expect to get the next frame soon, so we're waiting longer than usual.
            when (y === (totalHeightMax * 2).U) {
                // ... but not that long.
                regLocked := false.B
                startFrame := true.B
                regResyncWait := false.B
            }
        } .elsewhen (!regLocked && (y >= (totalHeightMin - 1).U)) {
            // Not locked, just refresh at the normal rate.
            startFrame := true.B
        } .elsewhen (y === (totalHeightMax - 1).U) {
            // Hit the maximum allowed total height without a new frame coming in:
            // source is too slow, switch to rapid refresh (no longer locked)
            regLocked := false.B
            startFrame := true.B
        }

        when ((y < (totalHeightMin - 1).U) && newFrameReady) {
            // New frame in the middle of drawing, skip that frame to resync.
            regResyncReady := true.B
        }
    } .otherwise {
        x := x + 1.U
        when (x === (hSync - 1).U) {
            regHsync := false.B
        }
        val isVActive = (y >= (vSync + vBackPorch).U) && (y < (vSync + vBackPorch + vActive).U)
        when (x === (hSync + hBackPorch - 1).U && isVActive) {
            regActive := true.B
        }
        when (x === (hSync + hBackPorch + hActive - 1).U) {
            regActive := false.B
        }
    }
}
