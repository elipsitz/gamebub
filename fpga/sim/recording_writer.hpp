#pragma once

#include <cstdint>

#include <unistd.h>
#include <filesystem>
#include <vector>

#include "framebuffer.hpp"

// Uses ffmpeg as a subprocess to output a video and audio recording.
//
// Spawns a separate ffmpeg for video and audio, and then combines them at the end.
class RecordingWriter {
  public:
    RecordingWriter(
        std::filesystem::path output_path,
        int video_width,
        int video_height,
        float video_framerate,
        int audio_freq
    );
    ~RecordingWriter();

    void write_video(Framebuffer& framebuffer);
    void write_audio(std::vector<int16_t>& samples);

  private:
    std::filesystem::path output_path_;
    pid_t video_pid_;
    pid_t audio_pid_;
    int video_pipe_;
    int audio_pipe_;
    std::string video_path_;
    std::string audio_path_;
};
