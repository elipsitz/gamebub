#pragma once

#include <filesystem>

#include "cartridge.hpp"
#include "framebuffer.hpp"
#include "input.hpp"
#include "VSimGameboy.h"

class Simulator {
public:
    Simulator(std::filesystem::path rom_path, std::filesystem::path bios_path);
    ~Simulator();

    void set_joypad_state(JoypadState state);
    void simulate_cycles(uint64_t cycles);
    void simulate_frame();
    void reset();
    Framebuffer& getFramebuffer() { return framebuffer; }
    std::vector<int16_t>& getAudioSampleBuffer();

    static int width() { return 160; }
    static int height() { return 144; }
    static int clockHz() { return 4 * 1024 * 1024; }
    static int audioSampleHz() { return 256 * 1024; }
    static float videoFramerate() { return 59.7275f; }

private:
    void stepFramebuffer();
    void stepAudio();

    Framebuffer framebuffer;

    std::unique_ptr<Cartridge> cart;
    uint64_t cycles = 0;
    VSimGameboy* top = nullptr;
    bool prev_lcd_enabled = false;
    bool is_cgb = true;
    std::vector<int16_t> audioSampleBuffer;
    int audioTimer = 0;
};
