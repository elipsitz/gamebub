package gba.ppu

import chisel3._
import chisel3.util._
import gba.ppu.PpuRegisters.AffineReferencePoint
import lib.log.Logger

class BackgroundPixel extends Bundle {
  // Whether the pixel is valid and opaque.
  val opaque = Bool()
  // Palette index (or, layer 2 and 3 in mode 3 and 5 combine to form a 16-bit color).
  val color = UInt(8.W)
}

class BackgroundScreenEntry extends Bundle {
  val paletteBank = UInt(4.W)
  val flipY = Bool()
  val flipX = Bool()
  val tile = UInt(10.W)
}

class BackgroundLayerState extends Bundle {
  /// Whether the layer is active during this part of the scanline.
  val active = Bool()
  /// Output pixel index.
  val pos = UInt(8.W)
  /// Step in the repeating 32-cycle rendering process.
  val cycle = UInt(5.W)
  /// For regular tilemaps, the screen entry.
  val screenEntry = new BackgroundScreenEntry()
  /// Queued up tile data (for regular tilemaps)
  val pixels = Vec(4, new BackgroundPixel)
}

class BackgroundRenderer extends Module {
  val io = IO(new Bundle {
    val enable = Input(Bool())

    val displayControl = Input(new PpuRegisters.DisplayControl)
    val bgControl = Input(Vec(4, new PpuRegisters.BackgroundControl))
    val bgOffX = Input(Vec(4, UInt(16.W)))
    val bgOffY = Input(Vec(4, UInt(16.W)))
    val bgAff = Input(Vec(2, new PpuRegisters.AffineParams))
    val bgAffX = Input(Vec(2, new PpuRegisters.AffineReferencePoint))
    val bgAffY = Input(Vec(2, new PpuRegisters.AffineReferencePoint))
    val writeAffX = Input(Vec(2, Bool()))
    val writeAffY = Input(Vec(2, Bool()))
    val mosaicY = Input(UInt(4.W))

    /// BG VRAM access
    val vram = Flipped(new PpuMemoryInterface(96 * 1024 / 2, 16.W))

    /// Current cycle in the scanline
    val tick = Input(UInt(11.W))
    val scanline = Input(UInt(8.W))

    /// Pixel fifo dequeue interface
    val pixels = Vec(4, DecoupledIO(new BackgroundPixel))
  })
  val logger = Logger("ppu.bg", enable = io.enable)

  // Output pixel FIFOs
  val fifo = (0 until 4).map(_ => Wire(EnqIO(new BackgroundPixel)))
  for (i <- 0 until 4) {
    fifo(i).valid := false.B
    fifo(i).bits := DontCare
  }
  val fifoFlush = WireDefault(false.B)
  io.pixels <> VecInit((0 until 4).map(i => Queue(fifo(i), entries = 5, flush = Some(fifoFlush))))

  // Per-layer state
  val layer = Reg(Vec(4, new BackgroundLayerState))
  val affX = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))
  val affY = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))
  val affXLine = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))
  val affYLine = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))
  val affXMosaic = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))
  val affYMosaic = Reg(Vec(2, new PpuRegisters.AffineReferencePoint))

  val mosaicCounter = Reg(UInt(4.W))

  val isVdraw = io.scanline < 160.U

  io.vram.read := false.B
  io.vram.address := DontCare

  switch (io.displayControl.mode) {
    is (0.U) {
      when (io.displayControl.enableBg(0)) { renderRegularLayer(0) }
      when (io.displayControl.enableBg(1)) { renderRegularLayer(1) }
      when (io.displayControl.enableBg(2)) { renderRegularLayer(2) }
      when (io.displayControl.enableBg(3)) { renderRegularLayer(3) }
    }
    is (1.U) {
      when (io.displayControl.enableBg(0)) { renderRegularLayer(0) }
      when (io.displayControl.enableBg(1)) { renderRegularLayer(1) }
      when (io.displayControl.enableBg(2)) { renderAffineLayer(2) }
    }
    is (2.U) {
      when (io.displayControl.enableBg(2)) { renderAffineLayer(2) }
      when (io.displayControl.enableBg(3)) { renderAffineLayer(3) }
    }
    is (3.U, 4.U, 5.U) {
      when (io.displayControl.enableBg(2)) { renderBitmapLayer() }
    }
  }

  when (io.enable && isVdraw) {
    for (i <- 0 until 4) {
      when (layer(i).active) {
        layer(i).cycle := layer(i).cycle + 1.U
      }
    }
    when (io.tick === 1005.U) {
      // Begin HBlank
      for (i <- 0 until 4) {
        layer(i).active := false.B
        layer(i).pos := 0.U
        layer(i).cycle := 0.U
      }
      for (i <- 0 until 2) {
        val newX = (affXLine(i).asUInt.asSInt + io.bgAff(i).pb.asUInt.asSInt).asTypeOf(new AffineReferencePoint)
        val newY = (affYLine(i).asUInt.asSInt + io.bgAff(i).pd.asUInt.asSInt).asTypeOf(new AffineReferencePoint)
        affX(i) := newX
        affY(i) := newY
        affXLine(i) := newX
        affYLine(i) := newY

        when (mosaicCounter === io.mosaicY) {
          affXMosaic(i) := newX
          affYMosaic(i) := newY
        } .elsewhen (io.bgControl(i + 2).mosaic) {
          affX(i) := affXMosaic(i)
          affY(i) := affYMosaic(i)
        }
      }
      fifoFlush := true.B

      mosaicCounter := mosaicCounter + 1.U
      when (mosaicCounter === io.mosaicY) {
        mosaicCounter := 0.U
      }
    }
  }

  // Update affine background params.
  when (io.enable) {
    when (isVdraw) {
      for (i <- 0 until 2) {
        when (io.writeAffX(i)) {
          affX(i) := io.bgAffX(i)
        }
        when (io.writeAffY(i)) {
          affY(i) := io.bgAffY(i)
        }
      }
    } .otherwise {
      affX := io.bgAffX
      affY := io.bgAffY
      affXLine := io.bgAffX
      affYLine := io.bgAffY
      affXMosaic := io.bgAffX
      affYMosaic := io.bgAffY
      mosaicCounter := 0.U
    }
  }

  private def renderRegularLayer(index: Int): Unit = {
    val control = io.bgControl(index)
    val state = layer(index)
    // Activate
    val startTick = (2 + index).U
    when (io.enable && isVdraw && io.tick === startTick) {
      state.active := true.B
    }

    // Render
    when (io.enable && state.active) {
      val x = state.pos + io.bgOffX(index) - 7.U
      val y = io.scanline + io.bgOffY(index) - Mux(control.mosaic, mosaicCounter, 0.U)
      val stage = x(2, 0)
      val fetch4bpp = (stage(1, 0) === 1.U) && !control.bpp8
      val fetch8bpp = (stage(0) === 1.U) && control.bpp8
      // Fetch
      when (state.cycle(1, 0) === 0.U) {
        when (stage === 0.U) {
          // Fetch map entry
          // 64KiB of vram for charblocks and screenblocks
          val tileX = x(7, 3)
          val tileY = y(7, 3)
          val screenOffset = VecInit(
            0.U(2.W), // 1x1
            x(8), // 2x1
            y(8), // 1x2
            Cat(y(8), x(8)),  // 2x2
          )
          val screenBlock = (control.screenBase + screenOffset(control.size))(4, 0)
          io.vram.read := true.B
          io.vram.address := Cat(screenBlock, tileY, tileX)
          // mapAddress should index 64KiB -- width should be 15.
        }
        when (fetch4bpp) {
          // Tiles are 32 bytes long
          // (16 bit full address)
          // (2 bits char block) (9 bit tile index (4bpp)) (5 bit byte)
          val col = stage(2) ^ state.screenEntry.flipX
          val row = Mux(state.screenEntry.flipY, ~y(2, 0), y(2, 0))
          val tile = Cat(control.charBase, 0.U(9.W)) +& state.screenEntry.tile
          val address = Cat(tile, row, col)
          io.vram.read := address < (32 * 1024).U // Reads above 64 KiB return open-bus.
          io.vram.address := address
        }
        when (fetch8bpp) {
          // Tiles are 64 bytes long
          val col = Mux(state.screenEntry.flipX, ~stage(2, 1), stage(2, 1))
          val row = Mux(state.screenEntry.flipY, ~y(2, 0), y(2, 0))
          val tile = Cat(control.charBase, 0.U(8.W)) +& state.screenEntry.tile
          val address = Cat(tile, row, col)
          io.vram.read := address < (32 * 1024).U // Reads above 64 KiB return open-bus.
          io.vram.address := address
        }

        state.pixels(0) := state.pixels(1)
        state.pixels(1) := state.pixels(2)
        state.pixels(2) := state.pixels(3)
        when (io.tick >= 39.U) {
          fifo(index).valid := true.B
          fifo(index).bits := state.pixels(0)
        }
      }
      // Use
      when (state.cycle(1, 0) === 1.U) {
        state.pos := state.pos + 1.U
        when (stage === 0.U) {
          state.screenEntry := io.vram.readData.asTypeOf(new BackgroundScreenEntry)
        }
        when (fetch4bpp) {
          for (i <- 0 until 4) {
            val data = Mux(
              state.screenEntry.flipX,
              io.vram.readData(4 * (3 - i) + 3, 4 * (3 - i)),
              io.vram.readData(4 * i + 3, 4 * i),
            )
            state.pixels(i).opaque := (data =/= 0.U)
            state.pixels(i).color := Cat(state.screenEntry.paletteBank, data)
          }
        }
        when (fetch8bpp) {
          for (i <- 0 until 2) {
            val data = Mux(
              state.screenEntry.flipX,
              io.vram.readData(8 * (1 - i) + 7, 8 * (1 - i)),
              io.vram.readData(8 * i + 7, 8 * i),
            )
            state.pixels(i).opaque := (data =/= 0.U)
            state.pixels(i).color := data
          }
        }
      }
    }
  }

  private def renderAffineLayer(index: Int): Unit = {
    val control = io.bgControl(index)
    val state = layer(index)
    val step = state.cycle(1, 0)
    val matrix = io.bgAff(index - 2)
    val refX = affX(index - 2)
    val refY = affY(index - 2)

    // Activate
    val startTick = if (index == 2) { 32 } else { 30 }
    when (io.enable && isVdraw && io.tick === startTick.U) {
      state.active := true.B
    }

    // Render
    when (io.enable && state.active) {
      val subtileX = refX.int(2, 0)
      val subtileY = refY.int(2, 0)

      when (step === 0.U) {
        // Fetch tile coordinate
        val screenBlock = Cat(control.screenBase, 0.U(11.W))
        val tileIndex = VecInit(
          Cat(0.U(3.W), refY.int(6, 3), refX.int(6, 3)),
          Cat(0.U(2.W), refY.int(7, 3), refX.int(7, 3)),
          Cat(0.U(1.W), refY.int(8, 3), refX.int(8, 3)),
          Cat(0.U(0.W), refY.int(9, 3), refX.int(9, 3)),
        )(control.size)
        val entry = screenBlock + tileIndex
        io.vram.read := true.B
        io.vram.address := entry >> 1
      }
      when (step === 1.U) {
        // Use tile coordinate to fetch data
        // Affine always uses 8bpp
        val tileIndex = Mux(
          refX.int(3),
          io.vram.readData(15, 8),
          io.vram.readData(7, 0),
        )
        val tile = Cat(control.charBase, 0.U(8.W)) +& tileIndex
        val address = Cat(tile, subtileY, subtileX)
        io.vram.read := true.B
        io.vram.address := address >> 1
      }
      when (step === 2.U) {
        // Use tile data
        refX := (refX.asUInt.asSInt + matrix.pa.asUInt.asSInt).asTypeOf(new AffineReferencePoint)
        refY := (refY.asUInt.asSInt + matrix.pc.asUInt.asSInt).asTypeOf(new AffineReferencePoint)

        val color = Mux(
          subtileX(0),
          io.vram.readData(15, 8),
          io.vram.readData(7, 0),
        )
        fifo(index).valid := true.B
        fifo(index).bits.opaque := color =/= 0.U
        fifo(index).bits.color := color

        when (!control.affineWrap) {
          val sizePx = (128.U << control.size).asUInt
          when (refX.sign.asBool || refY.sign.asBool || refX.int >= sizePx || refY.int >= sizePx) {
            fifo(index).bits.opaque := false.B
          }
        }
      }
    }
  }

  private def renderBitmapLayer(): Unit = {
    val state = layer(2)
    val step = state.cycle(1, 0)
    val matrix = io.bgAff(0)
    val refX = affX(0)
    val refY = affY(0)

    // Activate
    when (io.enable && isVdraw && io.tick === 33.U) {
      state.active := true.B
    }

    // Render
    when (io.enable && state.active) {
      // Fetch
      when (step === 0.U) {
        io.vram.read := true.B
        val frameOffset = Mux(io.displayControl.frame === 1.U, (0xA000 / 2).U, 0.U)
        // TODO determine if there's a good way to get rid of the multiply by width
        switch (io.displayControl.mode) {
          is (3.U) {
            // 240x160, 16bpp
            io.vram.address := ((refY.int * 240.U) + refX.int)
          }
          is (4.U) {
            // 240x160, indexed 8bpp
            io.vram.address := (((refY.int * 240.U) + refX.int) >> 1).asUInt.pad(16) + frameOffset
          }
          is (5.U) {
            // 160x128, 16bpp
            io.vram.address := ((refY.int * 160.U) + refX.int) + frameOffset
          }
        }
      }
      // Use
      when (step === 1.U) {
        refX := (refX.asUInt.asSInt + matrix.pa.asUInt.asSInt).asTypeOf(new AffineReferencePoint)
        refY := (refY.asUInt.asSInt + matrix.pc.asUInt.asSInt).asTypeOf(new AffineReferencePoint)

        fifo(2).valid := true.B
        fifo(2).bits.opaque := true.B

        switch (io.displayControl.mode) {
          is (4.U) {
            val color = Mux(
              refX.int(0) === 0.U,
              io.vram.readData(7, 0), io.vram.readData(15, 8)
            )
            fifo(2).bits.color := color
            when (color === 0.U) {
              fifo(2).bits.opaque := false.B
            }
          }
          is (3.U, 5.U) {
            fifo(3).valid := true.B
            fifo(3).bits.opaque := false.B

            fifo(2).bits.color := io.vram.readData(7, 0)
            fifo(3).bits.color := io.vram.readData(15, 8)
          }
        }

        // Clip sides (always)
        val widthPx = WireDefault(240.U)
        val heightPx = WireDefault(160.U)
        when (io.displayControl.mode === 5.U) {
          widthPx := 160.U
          heightPx := 128.U
        }
        when (refX.sign.asBool || refY.sign.asBool || refX.int >= widthPx || refY.int >= heightPx) {
          fifo(2).bits.opaque := false.B
        }
      }
    }
  }
}
