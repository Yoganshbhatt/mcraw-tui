#ifndef MC_C_API_H
#define MC_C_API_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque decoder handle */
typedef void McDecoder;

/* ------------------------------------------------------------------ */
/* Plain-data structs – safe to memcpy, no pointers inside            */
/* ------------------------------------------------------------------ */

typedef struct {
    uint32_t width;
    uint32_t height;
    double   white_level;
    double   black_level[4];
    int      black_level_count;   /* 1 or 4 */
    char     sensor_arrangement[8]; /* "rggb", "bggr", "grbg", "gbrg", or "" */
    float    color_matrix1[9];
    float    color_matrix2[9];
    float    forward_matrix1[9];
    float    forward_matrix2[9];

    /* DNG calibration illuminants (LightSource EXIF enum; 0 = unknown) */
    int32_t  calibration_illuminant1;
    int32_t  calibration_illuminant2;

    /* DNG calibration matrices (map camera native → reference camera space)     */
    /* These are optional – all-zero means identity.                              */
    float    calibration_matrix1[9];
    float    calibration_matrix2[9];

    /* Whether the container metadata was fully populated                       */
    bool     has_calibration_illuminants;

    int      audio_sample_rate_hz;
    int      num_audio_channels;
} McContainerMetadata;

typedef struct {
    uint32_t width;
    uint32_t height;
    int64_t  timestamp_ns;
    float    as_shot_neutral[3];
    double   exposure_time;
    float    iso;
    float    focal_length;
    float    aperture;
} McFrameMetadata;

/* ------------------------------------------------------------------ */
/* Lifecycle                                                          */
/* ------------------------------------------------------------------ */
McDecoder* decoder_create(const char* path);
void       decoder_destroy(McDecoder* decoder);

/* ------------------------------------------------------------------ */
/* Container-level metadata                                           */
/* ------------------------------------------------------------------ */
int decoder_get_container_metadata(McDecoder* decoder, McContainerMetadata* out);

/* ------------------------------------------------------------------ */
/* Frame index                                                        */
/* ------------------------------------------------------------------ */
int64_t decoder_get_frame_count(McDecoder* decoder);
int64_t decoder_get_frame_timestamps(McDecoder* decoder, int64_t* out_timestamps, int64_t capacity);

/* ------------------------------------------------------------------ */
/* Frame data                                                         */
/* ------------------------------------------------------------------ */
uint8_t* decoder_load_frame(McDecoder* decoder, int64_t timestamp_ns, uint32_t* out_size, McFrameMetadata* out_meta);
int      decoder_load_frame_metadata(McDecoder* decoder, int64_t timestamp_ns, McFrameMetadata* out_meta);

/* ------------------------------------------------------------------ */
/* Audio                                                              */
/* ------------------------------------------------------------------ */
int16_t* decoder_load_audio(McDecoder* decoder, uint32_t* out_sample_count);

/* ------------------------------------------------------------------ */
/* Memory management                                                  */
/* ------------------------------------------------------------------ */
void decoder_free_buffer(void* ptr);

/* ------------------------------------------------------------------ */
/* Error reporting                                                    */
/* ------------------------------------------------------------------ */
const char* decoder_last_error(void);

#ifdef __cplusplus
}
#endif

#endif /* MC_C_API_H */