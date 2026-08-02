"""Deterministic synthetic microscopy volumes for tests and benchmarks.

Generates OME-Zarr-shaped ``(t, c, z, y, x)`` uint16 volumes that mimic
fluorescence microscopy statistics — Gaussian blobs drifting over time,
z-correlated background, and Poisson shot noise — so nd-delta's cross-axis
decorrelation has realistic structure to exploit. Everything is seeded and
uses only :mod:`numpy`, so fixtures are reproducible across machines; the
real Tier 3 corpus (see ``docs/development/test-data.md``) is fetched
separately by ``scripts/fetch-bench-data.sh``.
"""

from __future__ import annotations

import numpy as np


def microscopy_volume(
    shape: tuple[int, int, int, int, int] = (8, 2, 32, 64, 64),
    seed: int = 7,
    blobs_per_channel: int = 12,
    noise_sigma: float | None = None,
) -> np.ndarray:
    """Return a ``(t, c, z, y, x)`` uint16 volume of drifting fluorescent blobs.

    The field is smooth along z and t (blobs drift slowly and span several
    slices) with per-voxel Poisson noise on top, which is the structure the
    nd-delta family is designed to decorrelate. Passing ``noise_sigma``
    replaces the Poisson shot noise with additive Gaussian noise of that
    σ (in counts) — see :func:`correlated_zstack`.
    """
    nt, nc, nz, ny, nx = shape
    rng = np.random.default_rng(seed)
    zz, yy, xx = np.meshgrid(
        np.arange(nz, dtype=np.float32),
        np.arange(ny, dtype=np.float32),
        np.arange(nx, dtype=np.float32),
        indexing="ij",
    )
    out = np.empty(shape, dtype=np.uint16)
    for c in range(nc):
        centers = rng.uniform(0.15, 0.85, size=(blobs_per_channel, 3)) * [nz, ny, nx]
        sigmas = rng.uniform(0.03, 0.10, size=(blobs_per_channel, 3)) * [nz, ny, nx]
        drift = rng.normal(0.0, 0.01, size=(blobs_per_channel, 3)) * [nz, ny, nx]
        amps = rng.uniform(0.3, 1.0, size=blobs_per_channel)
        # Smooth background: a diagonal gradient plus a gentle z modulation.
        background = (
            0.05 * (yy / ny + xx / nx)
            + 0.03 * np.sin(2.0 * np.pi * zz / max(nz, 2))
            + 0.05
        )
        for t in range(nt):
            field = background.copy()
            for k in range(blobs_per_channel):
                cz, cy, cx = centers[k] + t * drift[k]
                sz, sy, sx = sigmas[k]
                d2 = (
                    ((zz - cz) / sz) ** 2
                    + ((yy - cy) / sy) ** 2
                    + ((xx - cx) / sx) ** 2
                )
                field += amps[k] * np.exp(-0.5 * d2)
            photons = np.clip(field, 0.0, None) * 4000.0
            if noise_sigma is None:
                out[t, c] = rng.poisson(photons).astype(np.uint16)
            else:
                noisy = photons + rng.normal(0.0, noise_sigma, size=photons.shape)
                out[t, c] = np.clip(np.rint(noisy), 0, 65535).astype(np.uint16)
    return out


def correlated_zstack(
    shape: tuple[int, int, int, int, int] = (8, 2, 32, 128, 128),
    seed: int = 13,
) -> np.ndarray:
    """The Phase 2 decorrelation-gain fixture: a strongly z/t-correlated stack.

    The same drifting-blob field as :func:`microscopy_volume` but with photon
    shot noise suppressed to σ = 2 counts, so the smooth cross-slice structure
    the ``nd_lift`` transform captures — rather than per-voxel noise —
    dominates the chunk entropy. This is the fixture the
    ``nd-lift-delta-zstd``/``nd-lift-53-zstd`` lanes measure ratio gains on
    versus ``nd-delta-zstd``.
    """
    return microscopy_volume(shape=shape, seed=seed, noise_sigma=2.0)
