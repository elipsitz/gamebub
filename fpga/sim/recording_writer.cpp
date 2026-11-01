#include <unistd.h>
#include <sys/wait.h>
#include <cstdio>
#include <cstdlib>

#include "recording_writer.hpp"
#include "common.hpp"

namespace {

// Runs a subprocess, with a pipe to stdin.
void run_subprocess(const char** args, pid_t* out_pid, int* out_pipe) {
    int pipe_fds[2];
    if (pipe(pipe_fds) != 0) {
        perror("Failed to create pipe");
        std::exit(1);
    }

    pid_t pid = fork();
    if (pid < 0) {
        perror("Failed to fork");
        std::exit(1);
    } else if (pid == 0) {
        // Child process
        close(pipe_fds[1]);
        dup2(pipe_fds[0], STDIN_FILENO);
        close(pipe_fds[0]);

        execvp(args[0], const_cast<char* const *>(args));
        perror("Failed to exec");
        std::exit(1);
    }

    // Parent process
    close(pipe_fds[0]);
    *out_pid = pid;
    *out_pipe = pipe_fds[1];
}

void do_write_loop(int pipe, void* data, size_t to_write) {
    while (true) {
        ssize_t written = write(pipe, data, to_write);
        if (written < to_write) {
            to_write = to_write - (size_t)written;
        } else {
            break;
        }
    }
}

}

RecordingWriter::RecordingWriter(
    std::filesystem::path output_path,
    int video_width,
    int video_height,
    float video_framerate,
    int audio_freq
) : output_path_(output_path) {
    video_path_ = std::tmpnam(nullptr);
    audio_path_ = std::tmpnam(nullptr);
    printf("Temporary video output: %s\n", video_path_.c_str());
    printf("Temporary audio output: %s\n", audio_path_.c_str());

    // Launch video encoder
    std::string arg_resolution = string_format("%dx%d", video_width, video_height);
    std::string arg_framerate = string_format("%f", video_framerate);
    const char* args1[] = {
        "ffmpeg",
        "-hide_banner", "-loglevel", "error",
        // Input
        "-f", "rawvideo", "-pix_fmt", "bgra", "-s:v", arg_resolution.c_str(), "-r", arg_framerate.c_str(),
        "-color_range", "full",
        "-i", "-",
        // Output
        "-c:v", "libx264rgb", "-pix_fmt", "rgb24", "-color_range", "full", "-x264opts", "crf=0",
        "-f", "h264", video_path_.c_str(),
        //
        nullptr
    };
    run_subprocess(args1, &video_pid_, &video_pipe_);

    // Launch audio encoder
    std::string arg_audio_freq = string_format("%d", audio_freq);
    const char* args2[] = {
        "ffmpeg",
        "-hide_banner", "-loglevel", "error",
        "-f", "s16le", "-ar", arg_audio_freq.c_str(), "-ac", "2", "-i", "-",
        "-c:a", "flac", "-f", "flac", audio_path_.c_str(),
        nullptr
    };
    run_subprocess(args2, &audio_pid_, &audio_pipe_);
}

RecordingWriter::~RecordingWriter() {
    // Close pipes
    close(video_pipe_);
    close(audio_pipe_);

    // Wait for child ffmpeg to exit
    int status;
    waitpid(video_pid_, &status, 0);
    waitpid(audio_pid_, &status, 0);

    // Mux streams together
    pid_t mux_pid;
    int mux_pipe;
    const char* args[] = {
        "ffmpeg",
        "-hide_banner", "-loglevel", "error",
        "-i", video_path_.c_str(),
        "-i", audio_path_.c_str(),
        "-c", "copy",
        "-f", "matroska",
        "-y", output_path_.c_str(),
        nullptr
    };
    run_subprocess(args, &mux_pid, &mux_pipe);
    waitpid(mux_pid, &status, 0);
    printf("Muxed recorded video to %s\n", output_path_.c_str());

    std::remove(video_path_.c_str());
    std::remove(audio_path_.c_str());
}

void RecordingWriter::write_video(Framebuffer& framebuffer) {
    auto& buf = framebuffer.renderBuffer();
    do_write_loop(video_pipe_, buf.data(), buf.size());
}

void RecordingWriter::write_audio(std::vector<int16_t>& samples) {
    do_write_loop(audio_pipe_, samples.data(), samples.size() * 2);
}