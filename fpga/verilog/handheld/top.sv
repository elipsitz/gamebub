`default_nettype none

module top_handheld (
    input  wire clk_50mhz,

    input  wire        mcu_spi_clk,
    input  wire        mcu_spi_cs_n,
    inout  wire [3:0]  mcu_spi_d,
    output wire        mcu_irq_n,

    input  wire        btn_a,
    input  wire        btn_b,
    input  wire        btn_x,
    input  wire        btn_y,
    input  wire        btn_up,
    input  wire        btn_down,
    input  wire        btn_left,
    input  wire        btn_right,
    input  wire        btn_l,
    input  wire        btn_r,
    input  wire        btn_start,
    input  wire        btn_select,

    output wire        dac_mclk,
    output wire        dac_bclk,
    output wire        dac_wclk,
    output wire        dac_din,

    output wire        lcd_dotclk,
    output wire        lcd_hsync,
    output wire        lcd_vsync,
    output wire        lcd_data_en,
`ifdef BOARD_REV_4
    output wire [7:0]  lcd_data_r,
    output wire [7:0]  lcd_data_g,
    output wire [7:0]  lcd_data_b,
`else
    output wire [5:0]  lcd_data_r,
    output wire [5:0]  lcd_data_g,
    output wire [5:0]  lcd_data_b,
`endif
`ifdef BOARD_REV_2
    input wire         lcd_te,
`endif

    inout  wire [7:0]  cart_bank0,
    inout  wire [7:0]  cart_bank1,
    inout  wire [7:0]  cart_bank2,
    inout  wire [3:0]  cart_bank3,
    inout  wire        cart_pin30,
    output wire        cart_pin30_dir,
    inout  wire        cart_pin31,
    output wire        cart_pin31_dir,
    output wire        cart_bank0_dir,
    output wire        cart_bank1_dir,
    output wire        cart_bank2_dir,
    output wire        cart_bank3_dir,
    input  wire        cart_switch,
`ifdef BOARD_REV_1
    output wire        cart_en_3v3,
    output wire        cart_en_5v0,
`endif
`ifdef BOARD_REV_2
    output wire        cart_en_3v3,
    output wire        cart_en_5v0,
`endif
    output wire        cart_oe_n,

    inout  wire        link_so,
    inout  wire        link_si,
    inout  wire        link_sd,
    inout  wire        link_sc,
    output wire        link_so_dir,
    output wire        link_si_dir,
    output wire        link_sd_dir,
    output wire        link_sc_dir,

    output wire [17:0] sram_a,
    inout  wire [15:0] sram_io,
    output wire        sram_ce_n,
    output wire        sram_we_n,
    output wire        sram_oe_n,
    output wire        sram_ub_n,
    output wire        sram_lb_n,

    output wire        sdram_clk,
`ifdef BOARD_REV_4
    output wire [1:0]  sdram_cs_n,
    output wire [1:0]  sdram_cke,
`else
    output wire [0:0]  sdram_cs_n,
    output wire [0:0]  sdram_cke,
`endif
    output wire        sdram_ras_n,
    output wire        sdram_cas_n,
    output wire        sdram_we_n,
    output wire        sdram_ldqm,
    output wire        sdram_udqm,
    output wire [1:0]  sdram_bs,
    output wire [12:0] sdram_a,
    inout  wire [15:0] sdram_dq,

    inout  wire [3:0]  pmod,
`ifdef BOARD_REV_4
    inout  wire [3:0]  tp,
`endif

`ifdef BOARD_REV_1
    output wire        vibrate_en,
`elsif BOARD_REV_2
    output wire        vibrate_en,
    output wire        vibrate_brake_n,
`elsif BOARD_REV_3
    output wire        vibrate_en,
    output wire        vibrate_brake_n,
`else
    output wire        vibrate_in1,
    output wire        vibrate_in2,
`endif

`ifndef BOARD_REV_1
    inout  wire        usb_sbu_1,
    inout  wire        usb_sbu_2,
`endif

    output wire        hdmi_clk_p,
    output wire        hdmi_clk_n,
    output wire [2:0]  hdmi_data_p,
    output wire [2:0]  hdmi_data_n
);
    logic pll_reset = 1'd0;

    logic clk_sys;
    logic clk_sys_locked;
    logic clk_dpi;
    logic clk_hdmi;
    logic clk_hdmi_x5;
    logic clk_av;
    logic clk_hdmi_locked;
    logic clk_hdmi_power_down;

    // Manually construct IBUF for 50Mhz input clock to share between multiple MMCMs.
    wire clk_in_50mhz;
    IBUF clkin1_ibufg(
        .O (clk_in_50mhz),
        .I (clk_50mhz)
    );
    clk_wiz_hdmi_clk_wiz clk_wiz_hdmi(
        .reset(pll_reset),
        .locked(clk_hdmi_locked),
        .power_down(clk_hdmi_power_down),
        .clk_in_50mhz(clk_in_50mhz),
        .clk_out_hdmi(clk_hdmi),
        .clk_out_hdmi_x5(clk_hdmi_x5)
    );

    // Manual BUFH in case Vivado places the two MMCMs in different top/bottom halves
    wire clk_hdmi_bufh;
    BUFH bufh_av_hdmi (
        .O(clk_hdmi_bufh),
        .I(clk_hdmi)
    );
    // AV clock mux, select between clk_dpi and clk_hdmi
    logic hdmi_enable;
    BUFGMUX_CTRL bufgmux_av (
       .O(clk_av),
       .I0(clk_dpi),
       .I1(clk_hdmi_bufh),
       .S(hdmi_enable)
    );

    logic inner_cart_enabled;
    logic [7:0] inner_cart_bank0_in;
    logic [7:0] inner_cart_bank1_in;
    logic [7:0] inner_cart_bank2_in;
    logic [3:0] inner_cart_bank3_in;
    logic inner_cart_pin30_in;
    logic inner_cart_pin31_in;
    logic [7:0] inner_cart_bank0_out;
    logic [7:0] inner_cart_bank1_out;
    logic [7:0] inner_cart_bank2_out;
    logic [3:0] inner_cart_bank3_out;
    logic inner_cart_pin30_out;
    logic inner_cart_pin31_out;

    logic [3:0] inner_pmod_in;
    logic [3:0] inner_pmod_out;
    logic [3:0] inner_pmod_dir;

    logic inner_link_so_in;
    logic inner_link_si_in;
    logic inner_link_sd_in;
    logic inner_link_sc_in;
    logic inner_link_so_out;
    logic inner_link_si_out;
    logic inner_link_sd_out;
    logic inner_link_sc_out;

    logic inner_mcu_irq;
    logic [3:0] inner_mcu_spi_data_in;
    logic [3:0] inner_mcu_spi_data_out;
    logic [3:0] inner_mcu_spi_data_dir;

    logic [15:0] inner_sram_io_in;
    logic [15:0] inner_sram_io_out;
    logic inner_sram_io_dir;
    logic [1:0] inner_sram_write_mask;

    logic [1:0] inner_sdram_dqm;
    logic [15:0] inner_sdram_dq_in;
    logic [15:0] inner_sdram_dq_out;
    logic inner_sdram_dq_dir;

    logic hdmi_audio_clock;
    logic [15:0] hdmi_audio [1:0];
    logic [23:0] hdmi_rgb;
    logic [9:0] hdmi_cx;
    logic [9:0] hdmi_cy;

    logic inner_vibrate;

// Pins no longer connected, stub out.
`ifdef BOARD_REV_3
    logic cart_en_3v3;
    logic cart_en_5v0;
`endif
`ifdef BOARD_REV_4
    logic cart_en_3v3;
    logic cart_en_5v0;
`endif

    logic reset;
    pll_reset_generator sys_reset_gen(
        .clk(clk_sys),
        .clk_locked(clk_sys_locked),
        .reset(reset)
    );

    HandheldTop handheld_top(
        .clock(clk_sys),
        .reset(reset),

        .io_clockIn50Mhz(clk_in_50mhz),
        .io_clockOutSys(clk_sys),
        .io_clockOutDpi(clk_dpi),
        .io_clockOutLocked(clk_sys_locked),

        .io_clock_av(clk_av),

        .io_mcuIrq(inner_mcu_irq),
        .io_mcuSpiChipSelect(mcu_spi_cs_n),
        .io_mcuSpiClock(mcu_spi_clk),
        .io_mcuSpiDataIn(inner_mcu_spi_data_in),
        .io_mcuSpiDataOut(inner_mcu_spi_data_out),
        .io_mcuSpiDataDir(inner_mcu_spi_data_dir),

        .io_lcd_vsync(lcd_vsync),
        .io_lcd_hsync(lcd_hsync),
        .io_lcd_enable(lcd_data_en),
        .io_lcd_dotclk(lcd_dotclk),
        .io_lcdDataR(lcd_data_r),
        .io_lcdDataG(lcd_data_g),
        .io_lcdDataB(lcd_data_b),

        .io_dac_mclk(dac_mclk),
        .io_dac_wclk(dac_wclk),
        .io_dac_bclk(dac_bclk),
        .io_dac_data(dac_din),

        .io_hdmiEnable(hdmi_enable),
        .io_hdmiClockPowerDown(clk_hdmi_power_down),
        .io_hdmiAudioClock(hdmi_audio_clock),
        .io_hdmiAudio_0(hdmi_audio[0]),
        .io_hdmiAudio_1(hdmi_audio[1]),
        .io_hdmiRgb(hdmi_rgb),
        .io_hdmiCx(hdmi_cx),
        .io_hdmiCy(hdmi_cy),

        .io_buttons_a(btn_a),
        .io_buttons_b(btn_b),
        .io_buttons_x(btn_x),
        .io_buttons_y(btn_y),
        .io_buttons_up(btn_up),
        .io_buttons_down(btn_down),
        .io_buttons_left(btn_left),
        .io_buttons_right(btn_right),
        .io_buttons_l(btn_l),
        .io_buttons_r(btn_r),
        .io_buttons_start(btn_start),
        .io_buttons_select(btn_select),

        .io_cartridge3V3Enable(cart_en_3v3),
        .io_cartridge5V0Enable(cart_en_5v0),
        .io_cartridge_enabled(inner_cart_enabled),
        .io_cartridge_switch(cart_switch),
        .io_cartridge_bank0In(inner_cart_bank0_in),
        .io_cartridge_bank1In(inner_cart_bank1_in),
        .io_cartridge_bank2In(inner_cart_bank2_in),
        .io_cartridge_bank3In(inner_cart_bank3_in),
        .io_cartridge_pin30In(inner_cart_pin30_in),
        .io_cartridge_pin31In(inner_cart_pin31_in),
        .io_cartridge_bank0Out(inner_cart_bank0_out),
        .io_cartridge_bank1Out(inner_cart_bank1_out),
        .io_cartridge_bank2Out(inner_cart_bank2_out),
        .io_cartridge_bank3Out(inner_cart_bank3_out),
        .io_cartridge_pin30Out(inner_cart_pin30_out),
        .io_cartridge_pin31Out(inner_cart_pin31_out),
        .io_cartridge_bank0Dir(cart_bank0_dir),
        .io_cartridge_bank1Dir(cart_bank1_dir),
        .io_cartridge_bank2Dir(cart_bank2_dir),
        .io_cartridge_bank3Dir(cart_bank3_dir),
        .io_cartridge_pin30Dir(cart_pin30_dir),
        .io_cartridge_pin31Dir(cart_pin31_dir),

        .io_link_soIn(inner_link_so_in),
        .io_link_siIn(inner_link_si_in),
        .io_link_sdIn(inner_link_sd_in),
        .io_link_scIn(inner_link_sc_in),
        .io_link_soOut(inner_link_so_out),
        .io_link_siOut(inner_link_si_out),
        .io_link_sdOut(inner_link_sd_out),
        .io_link_scOut(inner_link_sc_out),
        .io_link_soDir(link_so_dir),
        .io_link_siDir(link_si_dir),
        .io_link_sdDir(link_sd_dir),
        .io_link_scDir(link_sc_dir),

        .io_sram_address(sram_a),
        .io_sram_dataIn(inner_sram_io_in),
        .io_sram_dataOut(inner_sram_io_out),
        .io_sram_dataDir(inner_sram_io_dir),
        .io_sram_ceN(sram_ce_n),
        .io_sram_weN(sram_we_n),
        .io_sram_oeN(sram_oe_n),
        .io_sram_writeMaskN(inner_sram_write_mask),

        .io_sdram_clock(sdram_clk),
`ifdef BOARD_REV_4
        .io_sdram_cke(sdram_cke),
        .io_sdram_cs(sdram_cs_n),
`else
        .io_sdram_cke(sdram_cke[0]),
        .io_sdram_cs(sdram_cs_n[0]),
`endif
        .io_sdram_ras(sdram_ras_n),
        .io_sdram_cas(sdram_cas_n),
        .io_sdram_we(sdram_we_n),
        .io_sdram_dqm(inner_sdram_dqm),
        .io_sdram_bank(sdram_bs),
        .io_sdram_address(sdram_a),
        .io_sdram_dataIn(inner_sdram_dq_in),
        .io_sdram_dataOut(inner_sdram_dq_out),
        .io_sdram_dataDir(inner_sdram_dq_dir),

        .io_pmod_in(inner_pmod_in),
        .io_pmod_out(inner_pmod_out),
        .io_pmod_dir(inner_pmod_dir),

        .io_vibrate(inner_vibrate)
    );

    assign cart_oe_n = ~inner_cart_enabled;
    assign inner_cart_bank0_in = cart_bank0;
    assign inner_cart_bank1_in = cart_bank1;
    assign inner_cart_bank2_in = cart_bank2;
    assign inner_cart_bank3_in = cart_bank3;
    assign inner_cart_pin30_in = cart_pin30;
    assign inner_cart_pin31_in = cart_pin31;
    assign cart_bank0 = cart_bank0_dir ? inner_cart_bank0_out : 8'hzz;
    assign cart_bank1 = cart_bank1_dir ? inner_cart_bank1_out : 8'hzz;
    assign cart_bank2 = cart_bank2_dir ? inner_cart_bank2_out : 8'hzz;
    assign cart_bank3 = cart_bank3_dir ? inner_cart_bank3_out : 8'hzz;
    assign cart_pin30 = cart_pin30_dir ? inner_cart_pin30_out : 1'bz;
    assign cart_pin31 = cart_pin31_dir ? inner_cart_pin31_out : 1'bz;

    assign inner_link_so_in = link_so;
    assign inner_link_si_in = link_si;
    assign inner_link_sd_in = link_sd;
    assign inner_link_sc_in = link_sc;
    assign link_so = link_so_dir ? inner_link_so_out : 1'bz;
    assign link_si = link_si_dir ? inner_link_si_out : 1'bz;
    assign link_sd = link_sd_dir ? inner_link_sd_out : 1'bz;
    assign link_sc = link_sc_dir ? inner_link_sc_out : 1'bz;

    assign inner_pmod_in = pmod;
    assign pmod[0] = inner_pmod_dir[0] ? inner_pmod_out[0] : 1'bz;
    assign pmod[1] = inner_pmod_dir[1] ? inner_pmod_out[1] : 1'bz;
    assign pmod[2] = inner_pmod_dir[2] ? inner_pmod_out[2] : 1'bz;
    assign pmod[3] = inner_pmod_dir[3] ? inner_pmod_out[3] : 1'bz;

`ifdef BOARD_REV_1
    assign vibrate_en = inner_vibrate;
`elsif BOARD_REV_2
    assign vibrate_en = inner_vibrate;
    assign vibrate_brake_n = 1'bz;
`elsif BOARD_REV_3
    assign vibrate_en = inner_vibrate;
    assign vibrate_brake_n = 1'bz;
`else
    assign vibrate_in1 = inner_vibrate;
    assign vibrate_in2 = 1'b0;
`endif

`ifdef BOARD_REV_1
    // Rev 1: FPGA irq directly connected to open-drain MCU_INT
    assign mcu_irq_n = inner_mcu_irq ? 1'b0 : 1'bz;
`else
    // Rev 2, 3, 4: FPGA irq connected to nFET, active-high
    assign mcu_irq_n = inner_mcu_irq;
`endif
    assign inner_mcu_spi_data_in = mcu_spi_d;
    assign mcu_spi_d[0] = inner_mcu_spi_data_dir[0] ? inner_mcu_spi_data_out[0] : 1'bz;
    assign mcu_spi_d[1] = inner_mcu_spi_data_dir[1] ? inner_mcu_spi_data_out[1] : 1'bz;
    assign mcu_spi_d[2] = inner_mcu_spi_data_dir[2] ? inner_mcu_spi_data_out[2] : 1'bz;
    assign mcu_spi_d[3] = inner_mcu_spi_data_dir[3] ? inner_mcu_spi_data_out[3] : 1'bz;

    assign inner_sram_io_in = sram_io;
    assign sram_io = inner_sram_io_dir ? inner_sram_io_out : 16'hzzzz;
    assign sram_ub_n = inner_sram_write_mask[1];
    assign sram_lb_n = inner_sram_write_mask[0];

    assign {sdram_udqm, sdram_ldqm} = inner_sdram_dqm;
    assign inner_sdram_dq_in = sdram_dq;
    assign sdram_dq = inner_sdram_dq_dir ? inner_sdram_dq_out : 16'hzzzz;

    // HDMI TMDS output
    logic hdmi_reset;
    pll_reset_generator hdmi_reset_gen(
        .clk(clk_hdmi),
        .clk_locked(clk_hdmi_locked),
        .reset(hdmi_reset)
    );
    // TODO: see if the OBUFTDS can be used: T must be connected to OSERDESE2 output
    logic hdmi_tmds_clock;
    logic [2:0] hdmi_tmds_data;
    hdmi #(
        // Video ID code 2: 720x480 @ 60.0Hz
        .VIDEO_ID_CODE(2),
        .VIDEO_REFRESH_RATE(60.00),
        .AUDIO_RATE(48000),
        .AUDIO_BIT_WIDTH(16),
        .PRODUCT_DESCRIPTION({"Game Bub", 64'd0}),
        .SOURCE_DEVICE_INFORMATION(8'h08)
    ) hdmi(
        // Pass clock_hdmi directly as the pixel clock, because the OSERDESE2 requires the two clocks
        // to pass through the same clocking resources (for phase alignment).
        .clk_pixel_x5(clk_hdmi_x5),
        .clk_pixel(clk_hdmi),
        .clk_audio(hdmi_audio_clock),
        .reset(hdmi_reset),
        .rgb(hdmi_rgb),
        .audio_sample_word(hdmi_audio),
        .tmds(hdmi_tmds_data),
        .tmds_clock(hdmi_tmds_clock),
        .cx(hdmi_cx),
        .cy(hdmi_cy)
    );
`ifdef BOARD_REV_1
    defparam hdmi.INVERT_D0 = 1;

    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds0      (.I(hdmi_tmds_data[0]), .O(hdmi_data_n[0]), .OB(hdmi_data_p[0]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds1      (.I(hdmi_tmds_data[1]), .O(hdmi_data_p[1]), .OB(hdmi_data_n[1]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds2      (.I(hdmi_tmds_data[2]), .O(hdmi_data_p[2]), .OB(hdmi_data_n[2]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds_clock (.I(hdmi_tmds_clock  ), .O(hdmi_clk_p    ), .OB(hdmi_clk_n    ));
`endif
`ifdef BOARD_REV_2
    defparam hdmi.INVERT_D0 = 1;
    defparam hdmi.INVERT_D1 = 1;
    defparam hdmi.INVERT_D2 = 1;
    defparam hdmi.INVERT_CLK = 1;

    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds0      (.I(hdmi_tmds_data[0]), .O(hdmi_data_n[0]), .OB(hdmi_data_p[0]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds1      (.I(hdmi_tmds_data[1]), .O(hdmi_data_n[1]), .OB(hdmi_data_p[1]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds2      (.I(hdmi_tmds_data[2]), .O(hdmi_data_n[2]), .OB(hdmi_data_p[2]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds_clock (.I(hdmi_tmds_clock  ), .O(hdmi_clk_n    ), .OB(hdmi_clk_p    ));
`endif
`ifdef BOARD_REV_3
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds0      (.I(hdmi_tmds_data[0]), .O(hdmi_data_p[0]), .OB(hdmi_data_n[0]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds1      (.I(hdmi_tmds_data[1]), .O(hdmi_data_p[1]), .OB(hdmi_data_n[1]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds2      (.I(hdmi_tmds_data[2]), .O(hdmi_data_p[2]), .OB(hdmi_data_n[2]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds_clock (.I(hdmi_tmds_clock  ), .O(hdmi_clk_p    ), .OB(hdmi_clk_n    ));
`endif
`ifdef BOARD_REV_4
    // TODO: this is due to the dock, not the handheld
    defparam hdmi.INVERT_D0 = 1;
    defparam hdmi.INVERT_D1 = 1;
    defparam hdmi.INVERT_D2 = 1;
    defparam hdmi.INVERT_CLK = 1;

    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds0      (.I(hdmi_tmds_data[0]), .O(hdmi_data_p[0]), .OB(hdmi_data_n[0]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds1      (.I(hdmi_tmds_data[1]), .O(hdmi_data_p[1]), .OB(hdmi_data_n[1]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds2      (.I(hdmi_tmds_data[2]), .O(hdmi_data_p[2]), .OB(hdmi_data_n[2]));
    OBUFDS #(.IOSTANDARD("TMDS_33")) obufds_clock (.I(hdmi_tmds_clock  ), .O(hdmi_clk_p    ), .OB(hdmi_clk_n    ));
`endif
endmodule

`default_nettype wire
