#pragma once

#include <cstdint>

#include <string>
#include <vector>

#include <SDL2/SDL.h>

#include "framebuffer.hpp"

class Window {
public:
    Window(int width, int height);
    ~Window();

    /// Update the window with the contents of the framebuffer.
    void update(Framebuffer& framebuffer);

    void setTitle(const char* title);
    
    const int SCALE = 2;

private:
    int width;
    int height;
    SDL_Window* window = nullptr;
    SDL_Renderer* renderer = nullptr;
    SDL_Texture* texture = nullptr;
};
