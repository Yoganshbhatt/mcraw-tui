#include "c_api.h"
#include <motioncam/Decoder.hpp>
#include <cstring>
#include <string>
#include <vector>
#include <memory>
#include <algorithm>
#include <stdexcept>
#include <cstdint>

static thread_local std::string tl_last_error;

static void set_error(const std::string& msg) {
    tl_last_error = msg;
}

template<typename T>
static T json_get(const nlohmann::json& j, const std::string& key, T fallback) {
    try {
        if (j.contains(key) && !j[key].is_null()) {
            return j[key].get<T>();
        }
    } catch (...) {}
    return fallback;
}

static int json_copy_float_array(const nlohmann::json& j, const std::string& key, float* dst, int max_len) {
    try {
        if (!j.contains(key) || !j[key].is_array()) return 0;
        const auto& arr = j[key];
        int count = std::min(static_cast<int>(arr.size()), max_len);
        for (int i = 0; i < count; ++i) dst[i] = arr[i].get<float>();
        return count;
    } catch (...) { return 0; }
}

static int json_copy_double_array(const nlohmann::json& j, const std::string& key, double* dst, int max_len) {
    try {
        if (!j.contains(key) || !j[key].is_array()) return 0;
        const auto& arr = j[key];
        int count = std::min(static_cast<int>(arr.size()), max_len);
        for (int i = 0; i < count; ++i) dst[i] = arr[i].get<double>();
        return count;
    } catch (...) { return 0; }
}

static void safe_strcpy(char* dst, const std::string& src, size_t dst_size) {
    if (dst_size == 0) return;
    std::strncpy(dst, src.c_str(), dst_size - 1);
    dst[dst_size - 1] = '\0';
}

static void fill_frame_metadata(McFrameMetadata* out, int64_t timestamp_ns, const nlohmann::json& meta) {
    if (!out) return;
    std::memset(out, 0, sizeof(*out));
    out->timestamp_ns   = timestamp_ns;
    out->width          = json_get<uint32_t>(meta, "width", 0);
    out->height         = json_get<uint32_t>(meta, "height", 0);
    out->exposure_time  = json_get<double>(meta, "exposureTime", 0.0);
    out->iso            = json_get<float>(meta, "iso", 0.0f);
    out->focal_length   = json_get<float>(meta, "focalLength", 0.0f);
    out->aperture       = json_get<float>(meta, "aperture", 0.0f);
    json_copy_float_array(meta, "asShotNeutral", out->as_shot_neutral, 3);
}

extern "C" {

McDecoder* decoder_create(const char* path) {
    if (!path) { set_error("decoder_create: path is NULL"); return nullptr; }
    try {
        auto* d = new motioncam::Decoder(std::string(path));
        return static_cast<McDecoder*>(d);
    } catch (const std::exception& e) {
        set_error(std::string("decoder_create: ") + e.what());
        return nullptr;
    } catch (...) {
        set_error("decoder_create: unknown exception");
        return nullptr;
    }
}

void decoder_destroy(McDecoder* decoder) {
    if (!decoder) return;
    delete static_cast<motioncam::Decoder*>(decoder);
}

int decoder_get_container_metadata(McDecoder* decoder, McContainerMetadata* out) {
    if (!decoder || !out) { set_error("decoder_get_container_metadata: NULL argument"); return -1; }
    try {
        auto* d = static_cast<motioncam::Decoder*>(decoder);
        const nlohmann::json& meta = d->getContainerMetadata();
        std::memset(out, 0, sizeof(*out));

        out->width  = json_get<uint32_t>(meta, "width", 0);
        out->height = json_get<uint32_t>(meta, "height", 0);
        out->white_level = json_get<double>(meta, "whiteLevel", 0.0);

        int bl_count = json_copy_double_array(meta, "blackLevel", out->black_level, 4);
        if (bl_count == 0) {
            try {
                if (meta.contains("blackLevel") && meta["blackLevel"].is_array()) {
                    const auto& bl = meta["blackLevel"];
                    bl_count = std::min(static_cast<int>(bl.size()), 4);
                    for (int i = 0; i < bl_count; ++i)
                        out->black_level[i] = static_cast<double>(bl[i].get<uint16_t>());
                }
            } catch (...) {}
        }
        out->black_level_count = bl_count;

        std::string arr = json_get<std::string>(meta, "sensorArrangment", "");
        if (arr.empty()) arr = json_get<std::string>(meta, "sensorArrangement", "");
        safe_strcpy(out->sensor_arrangement, arr, sizeof(out->sensor_arrangement));

        json_copy_float_array(meta, "colorMatrix1",   out->color_matrix1,   9);
        json_copy_float_array(meta, "colorMatrix2",   out->color_matrix2,   9);
        json_copy_float_array(meta, "forwardMatrix1", out->forward_matrix1, 9);
        json_copy_float_array(meta, "forwardMatrix2", out->forward_matrix2, 9);

        /* Calibration illuminants (DNG LightSource enum, 0 = unknown) */
        out->calibration_illuminant1 = json_get<int32_t>(meta, "calibrationIlluminant1", 0);
        out->calibration_illuminant2 = json_get<int32_t>(meta, "calibrationIlluminant2", 0);

        /* Optional calibration matrices */
        json_copy_float_array(meta, "calibrationMatrix1", out->calibration_matrix1, 9);
        json_copy_float_array(meta, "calibrationMatrix2", out->calibration_matrix2, 9);

        out->has_calibration_illuminants =
            (out->calibration_illuminant1 != 0) &&
            (out->calibration_illuminant2 != 0);

        out->audio_sample_rate_hz = d->audioSampleRateHz();
        out->num_audio_channels   = d->numAudioChannels();
        return 0;
    } catch (const std::exception& e) {
        set_error(std::string("decoder_get_container_metadata: ") + e.what());
        return -1;
    } catch (...) {
        set_error("decoder_get_container_metadata: unknown exception");
        return -1;
    }
}

int64_t decoder_get_frame_count(McDecoder* decoder) {
    if (!decoder) { set_error("decoder_get_frame_count: NULL decoder"); return -1; }
    try {
        return static_cast<int64_t>(static_cast<motioncam::Decoder*>(decoder)->getFrames().size());
    } catch (const std::exception& e) {
        set_error(std::string("decoder_get_frame_count: ") + e.what());
        return -1;
    } catch (...) {
        set_error("decoder_get_frame_count: unknown exception");
        return -1;
    }
}

int64_t decoder_get_frame_timestamps(McDecoder* decoder, int64_t* out_timestamps, int64_t capacity) {
    if (!decoder || !out_timestamps || capacity <= 0) {
        set_error("decoder_get_frame_timestamps: invalid arguments");
        return -1;
    }
    try {
        auto* d = static_cast<motioncam::Decoder*>(decoder);
        const auto& frames = d->getFrames();
        int64_t count = std::min(static_cast<int64_t>(frames.size()), capacity);
        for (int64_t i = 0; i < count; ++i)
            out_timestamps[i] = frames[static_cast<size_t>(i)];
        return count;
    } catch (const std::exception& e) {
        set_error(std::string("decoder_get_frame_timestamps: ") + e.what());
        return -1;
    } catch (...) {
        set_error("decoder_get_frame_timestamps: unknown exception");
        return -1;
    }
}

uint8_t* decoder_load_frame(McDecoder* decoder, int64_t timestamp_ns, uint32_t* out_size, McFrameMetadata* out_meta) {
    if (!decoder || !out_size) { set_error("decoder_load_frame: NULL argument"); return nullptr; }
    try {
        auto* d = static_cast<motioncam::Decoder*>(decoder);
        std::vector<uint8_t> data;
        nlohmann::json meta;
        d->loadFrame(static_cast<motioncam::Timestamp>(timestamp_ns), data, meta);

        if (data.empty()) { set_error("decoder_load_frame: frame data is empty"); return nullptr; }

        uint8_t* buf = new uint8_t[data.size()];
        std::memcpy(buf, data.data(), data.size());
        *out_size = static_cast<uint32_t>(data.size());
        fill_frame_metadata(out_meta, timestamp_ns, meta);
        return buf;
    } catch (const std::exception& e) {
        set_error(std::string("decoder_load_frame: ") + e.what());
        return nullptr;
    } catch (...) {
        set_error("decoder_load_frame: unknown exception");
        return nullptr;
    }
}

int decoder_load_frame_metadata(McDecoder* decoder, int64_t timestamp_ns, McFrameMetadata* out_meta) {
    if (!decoder || !out_meta) { set_error("decoder_load_frame_metadata: NULL argument"); return -1; }
    try {
        auto* d = static_cast<motioncam::Decoder*>(decoder);
        nlohmann::json meta;
        d->loadFrameMetadata(static_cast<motioncam::Timestamp>(timestamp_ns), meta);
        fill_frame_metadata(out_meta, timestamp_ns, meta);
        return 0;
    } catch (const std::exception& e) {
        set_error(std::string("decoder_load_frame_metadata: ") + e.what());
        return -1;
    } catch (...) {
        set_error("decoder_load_frame_metadata: unknown exception");
        return -1;
    }
}

int16_t* decoder_load_audio(McDecoder* decoder, uint32_t* out_sample_count) {
    if (!decoder || !out_sample_count) { set_error("decoder_load_audio: NULL argument"); return nullptr; }
    try {
        auto* d = static_cast<motioncam::Decoder*>(decoder);
        std::vector<motioncam::AudioChunk> chunks;
        d->loadAudio(chunks);

        size_t total = 0;
        for (const auto& chunk : chunks) total += chunk.second.size();
        if (total == 0) { set_error("decoder_load_audio: no audio data"); return nullptr; }

        // Allocate as uint8_t[] to match decoder_free_buffer's delete[] cast
        size_t total_bytes = total * sizeof(int16_t);
        uint8_t* buf = new uint8_t[total_bytes];
        int16_t* audio_ptr = reinterpret_cast<int16_t*>(buf);
        
        size_t pos = 0;
        for (const auto& chunk : chunks) {
            std::memcpy(audio_ptr + pos, chunk.second.data(), chunk.second.size() * sizeof(int16_t));
            pos += chunk.second.size();
        }

        *out_sample_count = static_cast<uint32_t>(total);
        return audio_ptr;
    } catch (const std::exception& e) {
        set_error(std::string("decoder_load_audio: ") + e.what());
        return nullptr;
    } catch (...) {
        set_error("decoder_load_audio: unknown exception");
        return nullptr;
    }
}

void decoder_free_buffer(void* ptr) {
    if (ptr) delete[] static_cast<uint8_t*>(ptr);
}

const char* decoder_last_error(void) {
    return tl_last_error.c_str();
}

} /* extern "C" */