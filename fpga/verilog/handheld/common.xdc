########################################
# Config
########################################
# Enable bitstream compression
set_property BITSTREAM.GENERAL.COMPRESS True [current_design]

# Tell Vivado the voltages used during configuration
set_property CFGBVS VCCO [current_design]
set_property CONFIG_VOLTAGE 3.3 [current_design]

# Configure framebuffers with "power-optimized" ram-decomposition.
#
# This affects how Vivado elaborates RAMs that are larger than a RAMB36: by default,
# it'll optimize for timing, which means splitting entries across multiple BRAMs.
# This is problematic if the number of rows isn't a power of two, because it'll waste
# a significant amount of space (e.g. a 240x160 framebuffer, with 38,400 15-bit entries,
# will end up using 30 BRAMs (each 1 bit x 32768) (with expansion).
#
# Power optimized decomposition will split rows across BRAMs, so the same framebuffer will
# use 19 BRAMs (each 15-bit x 2048) (depth expansion).
#
# -quiet prevents Vivado from complaining about get_cells returning nothing during implementation
# (since this is a synthesis-only directory). However, it also masks any errors due to the
# path actually being wrong in synthesis.
set_property -quiet ram_decomp power [get_cells handheld_top/overlayFramebuffer_sram_ext/Memory_reg]
set_property -quiet ram_decomp power [get_cells handheld_top/framebuffers_sram_ext/Memory_reg]
set_property -quiet ram_decomp power [get_cells handheld_top/framebuffers_sram_*_ext/Memory_reg]

########################################
# Clocking
########################################
# External 50 MHz input clock
create_clock -add -name sys_clk_pin -period 20.00 -waveform {0 10} [get_ports { clk_50mhz }];
set_property CLOCK_DEDICATED_ROUTE FALSE [get_nets clk_in_50mhz]

# The 'dpi' and 'hdmi' clocks pass through a clock mux, and are mutually exclusive.
# Mark their fanouts as being in a physically exclusive clock group.
# Query the clocks directly from the multiplexer pins so cores do not need to name their clock net 'clk_dpi'.
set_clock_groups -name exclusive_dpi_hdmi -physically_exclusive \
    -group [get_clocks -of_objects [get_pins -hierarchical *bufgmux_av/I0]] \
    -group [get_clocks -of_objects [get_pins -hierarchical *bufgmux_av/I1]]

# Mark the BUFGMUX_CTRL select input as having a false path.
# From UG472, the select input's setup/hold times only determine whether the old clock is used for an extra cycle
# after changing, which we don't care about.
set_false_path -setup -hold -to [get_pins -hierarchical *bufgmux_av/S0]
set_false_path -setup -hold -to [get_pins -hierarchical *bufgmux_av/S1]
