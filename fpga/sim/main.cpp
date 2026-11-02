#include <iostream>
#include <format>
#include <vector>
#include <cstdio>
#include <cmath>
#include <getopt.h>
#include <optional>
#include <fstream>
#include <filesystem>
#include <sstream>
#include <string>

#include <SDL2/SDL.h>

#include "audio.hpp"
#include "simulator.hpp"
#include "window.hpp"
#include "recording_writer.hpp"

JoypadState read_joypad_state() {
    const uint8_t* keyboard = SDL_GetKeyboardState(nullptr);
    JoypadState joypad = {};
    joypad.start = keyboard[SDL_SCANCODE_RETURN];
    joypad.select = keyboard[SDL_SCANCODE_RSHIFT];
    joypad.b = keyboard[SDL_SCANCODE_X];
    joypad.a = keyboard[SDL_SCANCODE_Z];
    joypad.down = keyboard[SDL_SCANCODE_DOWN];
    joypad.up = keyboard[SDL_SCANCODE_UP];
    joypad.left = keyboard[SDL_SCANCODE_LEFT];
    joypad.right = keyboard[SDL_SCANCODE_RIGHT];
    joypad.l = keyboard[SDL_SCANCODE_A];
    joypad.r = keyboard[SDL_SCANCODE_S];
    return joypad;
}

void print_help() {
    std::cout << "Usage: sim"
        << " --bios path/to/bios"
        << " [--headless --frames N] [--screenshot out.ppm]"
        << " [--input frame:buttons:frames,...]"
        << " [rom path]\n"
        << "  buttons: '+'-separated list of a,b,start,select,up,down,left,right,l,r\n";
    std::exit(1);
}

// A scripted button press, for headless runs.
struct ScriptedInput {
    int frame;
    int duration;
    JoypadState state;
};

std::vector<ScriptedInput> parse_input_script(const std::string& script) {
    std::vector<ScriptedInput> inputs;
    std::stringstream items(script);
    std::string item;
    while (std::getline(items, item, ',')) {
        std::stringstream parts(item);
        std::string frame, buttons, duration = "5";
        std::getline(parts, frame, ':');
        std::getline(parts, buttons, ':');
        std::getline(parts, duration, ':');
        ScriptedInput input = {std::stoi(frame), std::stoi(duration), {}};
        std::stringstream names(buttons);
        std::string name;
        while (std::getline(names, name, '+')) {
            if (name == "a") input.state.a = true;
            else if (name == "b") input.state.b = true;
            else if (name == "start") input.state.start = true;
            else if (name == "select") input.state.select = true;
            else if (name == "up") input.state.up = true;
            else if (name == "down") input.state.down = true;
            else if (name == "left") input.state.left = true;
            else if (name == "right") input.state.right = true;
            else if (name == "l") input.state.l = true;
            else if (name == "r") input.state.r = true;
            else {
                std::cerr << "Unknown button: " << name << std::endl;
                std::exit(1);
            }
        }
        inputs.push_back(input);
    }
    return inputs;
}

JoypadState scripted_joypad_state(const std::vector<ScriptedInput>& inputs, int frame) {
    JoypadState state = {};
    for (const auto& input : inputs) {
        if (frame >= input.frame && frame < input.frame + input.duration) {
            state.a |= input.state.a;
            state.b |= input.state.b;
            state.start |= input.state.start;
            state.select |= input.state.select;
            state.up |= input.state.up;
            state.down |= input.state.down;
            state.left |= input.state.left;
            state.right |= input.state.right;
            state.l |= input.state.l;
            state.r |= input.state.r;
        }
    }
    return state;
}

// Write a BGRA framebuffer as a binary PPM image.
void write_ppm(const std::filesystem::path& path, const std::vector<uint8_t>& bgra, int width, int height) {
    std::ofstream out(path, std::ios::binary);
    out << "P6\n" << width << " " << height << "\n255\n";
    for (int i = 0; i < width * height; i++) {
        const char rgb[3] = {(char)bgra[i * 4 + 2], (char)bgra[i * 4 + 1], (char)bgra[i * 4 + 0]};
        out.write(rgb, 3);
    }
}

int main(int argc, char** argv) {
    std::filesystem::path bios_path;
    std::filesystem::path rom_path;
    std::filesystem::path audio_save_path;
    std::filesystem::path record_path;
    std::filesystem::path screenshot_path;
    std::vector<ScriptedInput> input_script;
    bool headless = false;
    long max_frames = -1;

    const char* const short_opts = "b:";
    const option long_opts[] = {
        {"bios-path", required_argument, nullptr, 'b'},
        {"save-audio", required_argument, nullptr, 'a'},
        {"record", required_argument, nullptr, 'r'},
        {"headless", no_argument, nullptr, 'H'},
        {"frames", required_argument, nullptr, 'f'},
        {"screenshot", required_argument, nullptr, 's'},
        {"input", required_argument, nullptr, 'i'},
        {"help", no_argument, nullptr, 'h'},
        {nullptr, no_argument, nullptr, 0}
    };

    while (true) {
        const auto opt = getopt_long(argc, argv, short_opts, long_opts, nullptr);
        if (opt == -1) {
            break;
        }

        switch (opt) {
            case 'b':
                bios_path = optarg;
                break;
            case 'a':
                audio_save_path = optarg;
                break;
            case 'r':
                record_path = optarg;
                break;
            case 'H':
                headless = true;
                break;
            case 'f':
                max_frames = std::stol(optarg);
                break;
            case 's':
                screenshot_path = optarg;
                break;
            case 'i':
                input_script = parse_input_script(optarg);
                break;
            case 'h':
            case '?':
            default:
                print_help();
                break;
        }
    }
    if (optind < argc) {
        rom_path = argv[optind++];
    }
    if (rom_path.empty()) {
        print_help();
    }

    if (headless && max_frames < 0) {
        std::cerr << "--headless requires --frames" << std::endl;
        std::exit(1);
    }

    Simulator simulator(rom_path, bios_path);
    simulator.reset();

    if (headless) {
        for (long frame = 0; frame < max_frames; frame++) {
            simulator.set_joypad_state(scripted_joypad_state(input_script, frame));
            simulator.simulate_frame();
            simulator.getAudioSampleBuffer().clear();
        }
        if (!screenshot_path.empty()) {
            write_ppm(screenshot_path, simulator.getFramebuffer().renderBuffer(),
                Simulator::width(), Simulator::height());
        }
        return 0;
    }

    // Initialize SDL.
    SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS | SDL_INIT_AUDIO);
    Window window(Simulator::width(), Simulator::height());
    Audio audio(audio_save_path);

    std::optional<RecordingWriter> recording;
    if (!record_path.empty()) {
        recording.emplace(
            record_path,
            Simulator::width(),
            Simulator::height(),
            Simulator::videoFramerate(),
            Simulator::audioSampleHz()
        );
    }

    bool single_step = false;
    bool paused = false;
    uint64_t frame_timer = 0;
    int frame_counter = 0;
    while (true) {
        // Handle SDL events.
        SDL_Event event;
        if (SDL_PollEvent(&event)) {
            if (event.type == SDL_QUIT) {
                break;
            } else if (event.type == SDL_KEYDOWN) {
                auto key = event.key.keysym;
                bool command = (key.mod & KMOD_GUI) != 0;
                if (key.sym == SDLK_p && command) {
                    paused = !paused;
                } else if (key.sym == SDLK_n && command) {
                    paused = true;
                    single_step = true;
                } else if (key.sym == SDLK_r && command) {
                    std::cout << "Resetting..." << std::endl;
                    simulator.reset();
                }
            }
        }

        // Simulate for a frame.
        if (!paused || single_step) {
            // Simulate
            simulator.set_joypad_state(read_joypad_state());
            simulator.simulate_frame();
            frame_counter++;

            if (recording.has_value()) {
                recording->write_video(simulator.getFramebuffer());
                recording->write_audio(simulator.getAudioSampleBuffer());
            }

            // Audio
            std::vector<int16_t>& samples = simulator.getAudioSampleBuffer();
            audio.push(samples.data(), samples.size());
            samples.clear();
        }
        window.update(simulator.getFramebuffer());
        single_step = false;

        // Update title.
        if (SDL_GetTicks64() - frame_timer >= 1000) {
            char buffer[100];
            snprintf(buffer, 100, "Sim - FPS: %d", frame_counter);
            window.setTitle(buffer);
            frame_counter = 0;
            frame_timer = SDL_GetTicks64();
        }
    }

    SDL_Quit();
}

// Needed for Verilator with some linkers.
double sc_time_stamp() { return 0; }
