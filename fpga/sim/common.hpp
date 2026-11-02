#pragma once

#include <cstdint>

#include <vector>
#include <filesystem>
#include <iostream>
#include <fstream>
#include <memory>
#include <string>
#include <stdexcept>

// https://stackoverflow.com/a/26221725
template<typename ... Args>
std::string string_format( const std::string& format, Args ... args )
{
    int size_s = std::snprintf(nullptr, 0, format.c_str(), args...) + 1; // Extra space for '\0'
    if (size_s <= 0) {
        throw std::runtime_error("Error during formatting.");
    }
    auto size = static_cast<size_t>(size_s);
    std::unique_ptr<char[]> buf(new char[size]);
    std::snprintf(buf.get(), size, format.c_str(), args...);
    return std::string(buf.get(), buf.get() + size - 1); // We don't want the '\0' inside
}


static std::vector<uint8_t> read_file(std::filesystem::path path) {
    std::vector<uint8_t> buffer;
    std::ifstream in(path, std::ios::binary);
    in.seekg(0, std::ios::end);
    size_t size = in.tellg();
    in.seekg(0, std::ios::beg);
    buffer.resize(size);
    in.read(reinterpret_cast<char*>(buffer.data()), size);
    return buffer;
}

static void write_file(std::filesystem::path path, std::vector<uint8_t>& buffer) {
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    out.write(reinterpret_cast<char*>(buffer.data()), buffer.size());
}