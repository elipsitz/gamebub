#pragma once

#include <cstdint>

#include <vector>

class Framebuffer {
  public:
    Framebuffer(int width, int height);

    void update(bool hblank, bool vblank);
    void clear();
    void pushBGR(uint16_t pixel);

    std::vector<uint8_t>& renderBuffer();

  private:
    std::vector<uint8_t>& writeBuffer();

    int width;
    int height;

    std::vector<uint8_t> page0;
    std::vector<uint8_t> page1;
    int activePage = 0;

    bool prev_vblank = false;
    bool prev_hblank = false;

    int x = 0;
    int y = 0;
};
