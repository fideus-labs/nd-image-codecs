// Differential-test vector generator for the ndic-htj2k block coder.
//
// Generates random code-blocks, runs OpenJPH's reference 32-bit HT block
// encoder and decoder on them, and writes binary records the Rust test
// `openjph_differential.rs` consumes:
//
//   record := u32 width, height, k_max, stripe_causal, coded_len
//             u32 samples[width*height]      (sign-magnitude input)
//             u8  coded[coded_len]           (cleanup segment)
//             u32 decoded[width*height]      (reference decode output)
//
// All u32 little-endian. Build & run: see scripts/ht-differential.sh.

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#include "ojph_arch.h"
#include "ojph_mem.h"
#include "ojph_block_encoder.h"
#include "ojph_block_decoder.h"

using namespace ojph;
using namespace ojph::local;

static uint64_t lcg(uint64_t &s) {
  s = s * 6364136223846793005ULL + 1442695040888963407ULL;
  return s >> 32;
}

int main(int argc, char **argv) {
  int n = argc > 1 ? atoi(argv[1]) : 1000;
  const char *out_path = argc > 2 ? argv[2] : "ht_vectors.bin";

  initialize_block_encoder_tables();
  FILE *f = fopen(out_path, "wb");
  if (!f) { perror("fopen"); return 1; }

  mem_elastic_allocator elastic(1 << 22);
  int written = 0;
  for (int i = 0; written < n; ++i) {
    uint64_t s = 0x9E3779B97F4A7C15ULL ^ (uint64_t)i;
    ui32 width = 1 + (ui32)(lcg(s) % 64);
    ui32 height = 1 + (ui32)(lcg(s) % 64);
    ui32 k_max = 2 + (ui32)(lcg(s) % 27); // 2..=28
    ui32 density = (ui32)(lcg(s) % 101);
    ui32 causal = (ui32)(lcg(s) % 2);
    ui32 shift = 31 - k_max;

    std::vector<ui32> buf(width * height, 0);
    ui32 mv = 0;
    for (ui32 j = 0; j < width * height; ++j) {
      if (lcg(s) % 100 < density) {
        int64_t bound = 1LL << (k_max - 1);
        int64_t m = (int64_t)(lcg(s) % (ui64)(2 * bound)) - bound;
        ui32 sign = m < 0 ? 0x80000000u : 0;
        ui32 mag = (ui32)(m < 0 ? -m : m) << shift;
        buf[j] = sign | mag;
        mv |= mag;
      }
    }
    if (mv < (1u << shift))
      continue; // block would not be coded at all

    ui32 lengths[2] = {0, 0};
    coded_lists *coded = NULL;
    ojph_encode_codeblock32(buf.data(), k_max - 1, 1, width, height, width,
                            lengths, &elastic, coded);

    // Decode from a padded copy: the reference decoder deliberately reads a
    // few bytes around the segment.
    std::vector<ui8> padded(lengths[0] + 32, 0);
    memcpy(padded.data() + 16, coded->buf, lengths[0]);
    std::vector<ui32> dec((height + 2) * width, 0);
    bool ok = ojph_decode_codeblock32(padded.data() + 16, dec.data(),
                                      k_max - 1, 1, lengths[0], 0, width,
                                      height, width, causal != 0);
    if (!ok) {
      fprintf(stderr, "reference decode failed on vector %d\n", i);
      return 2;
    }

    ui32 hdr[5] = {width, height, k_max, causal, lengths[0]};
    fwrite(hdr, 4, 5, f);
    fwrite(buf.data(), 4, width * height, f);
    fwrite(coded->buf, 1, lengths[0], f);
    fwrite(dec.data(), 4, width * height, f);
    ++written;
  }
  fclose(f);
  fprintf(stderr, "wrote %d vectors to %s\n", written, out_path);
  return 0;
}
