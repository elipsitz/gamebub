#include <iostream>

#include "audio.hpp"
#include "common.hpp"
#include "simulator.hpp"

#include "VSimGameboy___024root.h"

// "hollow knight inspired" palette
//static const uint32_t palette[4] = {0xfafbf6, 0xc6b7be, 0x565a75, 0x0f0f1b};
// gray palette
//static const uint32_t palette[4] = {0xffffff, 0xaaaaaa, 0x555555, 0x000000};

Simulator::Simulator(std::filesystem::path rom_path, std::filesystem::path bios_path)
    : framebuffer(width(), height())
{
    this->cart = std::make_unique<Cartridge>(rom_path);
    this->top = new VSimGameboy;

    if (bios_path.empty()) {
        std::cerr << "ERROR: must specify bios path\n";
        std::exit(1);
    }
    auto bios = read_file(bios_path);
    if (bios.size() != 256 && bios.size() != 2304) {
        std::cerr << "ERROR: incorrect bios size: " << bios.size() << " (expected 256 or 2304)\n";
        std::exit(1);
    }
    // The model is determined by the boot ROM.
    is_cgb = bios.size() == 2304;
    top->io_isCgb = is_cgb;

    // Note: assumes little-endian
    memcpy(
        &this->top->rootp->SimGameboy__DOT__bootRom_ext__DOT__Memory,
        bios.data(),
        bios.size()
    );
}

Simulator::~Simulator()
{
    top->final();
    delete top;
}

void Simulator::reset()
{
    top->io_clockConfig_enable = true;
    top->io_dataAccess_dataRead = 0;
    top->io_cartConfig_mbcType = cart->mbc_type;
    top->io_cartConfig_hasRam = cart->has_ram;
    top->io_cartConfig_hasRtc = cart->has_timer;
    top->io_cartConfig_hasRumble = cart->has_rumble;
    top->reset = 1;

    uint64_t total = 8 - (cycles % 8);
    simulate_cycles(total);

    top->reset = 0;
}

void Simulator::set_joypad_state(JoypadState state)
{
    top->io_joypad_start = state.start;
    top->io_joypad_select = state.select;
    top->io_joypad_b = state.b;
    top->io_joypad_a = state.a;
    top->io_joypad_down = state.down;
    top->io_joypad_up = state.up;
    top->io_joypad_left = state.left;
    top->io_joypad_right = state.right;
}

void Simulator::simulate_cycles(uint64_t num_cycles)
{
    for (uint64_t i = 0; i < num_cycles * 2; i++) {
        top->io_clockConfig_provide8Mhz = top->io_clockConfig_need8Mhz;
        if (!top->io_clockConfig_provide8Mhz) {
            i++;
        }

        // Handle memory.
        if (top->io_dataAccess_enable) {
            std::vector<uint8_t>& mem = top->io_dataAccess_selectRom ? cart->rom : cart->ram;

            if (mem.empty()) {
                // No cartridge RAM.
                top->io_dataAccess_dataRead = 0xFF;
            } else if (top->io_dataAccess_write) {
                mem[top->io_dataAccess_address % mem.size()] = top->io_dataAccess_dataWrite;
            } else {
                top->io_dataAccess_dataRead = mem[top->io_dataAccess_address % mem.size()];
            }
            top->io_dataAccess_valid = true;
        }

        this->stepFramebuffer();
        this->stepAudio();

        top->clock = 0;
        top->eval();
        top->clock = 1;
        top->eval();

        this->cycles++;
    }
}

void Simulator::stepFramebuffer()
{
    framebuffer.update(top->io_ppu_hblank, top->io_ppu_vblank);

    if (top->io_ppu_valid && top->io_ppu_lcdEnable) {
      if (is_cgb) {
        framebuffer.pushBGR(top->io_ppu_pixel);
      } else {
        // DMG: grayscale shades (color 0 is dark, 3 is bright).
        uint16_t shade = top->io_ppu_dmgColor_color * 10 + 1;
        framebuffer.pushBGR(shade | (shade << 5) | (shade << 10));
      }
    }

    // Blank the screen if the LCD is disabled.
    if (prev_lcd_enabled && !top->io_ppu_lcdEnable) {
      framebuffer.clear();
    }
    prev_lcd_enabled = top->io_ppu_lcdEnable;
}

void Simulator::simulate_frame()
{
    simulate_cycles(70224);
}

void Simulator::stepAudio()
{
    int interval = clockHz() / audioSampleHz();
    if (top->io_clockConfig_provide8Mhz) {
        interval *= 2;
    }

    audioTimer++;
    if (audioTimer == interval) {
        int16_t mask = 1U << (10 - 1);
        int16_t left = (top->io_apu_left ^ mask) - mask;
        int16_t right = (top->io_apu_right ^ mask) - mask;

        audioTimer = 0;
        audioSampleBuffer.push_back(left * 8);
        audioSampleBuffer.push_back(right * 8);
    }
}

std::vector<int16_t>& Simulator::getAudioSampleBuffer()
{
    return audioSampleBuffer;
}
