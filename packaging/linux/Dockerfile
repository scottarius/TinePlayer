# The machine a Linux package is built on, so that it does not matter whose
# machine actually runs the build.
#
# Debian 12 on purpose. A .deb records the versions of the libraries it was
# linked against, and apt will not install it on a system with older ones, so
# building here sets the oldest system the package will install on. Debian 12
# is what Raspberry Pi OS is built from, and is old enough that Ubuntu 22.04
# and everything newer are covered too. Building on something newer would
# quietly leave the Pi behind, which for this application is the wrong end to
# lose.
#
# Both architectures use this same file. On an x86_64 machine, Docker runs the
# arm64 variant under emulation, which is slow but produces exactly what a Pi
# needs without owning a second build machine:
#
#   docker build --platform linux/arm64 -f packaging/linux/Dockerfile -t tine-deb-arm64 .
#   docker run --rm --platform linux/arm64 -v "$PWD:/src" -w /src tine-deb-arm64 \
#       ./packaging/linux/package.sh
#
# No --privileged and no emulation caveats: building a .deb is a compile and a
# file copy, so nothing here needs the kernel namespaces that a sandboxed
# build system would.
FROM debian:12

# Everything needed to compile against GTK and GStreamer, and to build a
# package from the result. The GStreamer plugin packages are not here: they
# are what the finished package depends on at runtime, not what it is built
# against.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    dpkg-dev \
    lintian \
    fakeroot \
    file \
    curl \
    ca-certificates \
    libgtk-4-dev \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libgstreamer-plugins-bad1.0-dev \
    meson \
    ninja-build \
    nasm \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# dav1d, built here rather than taken from Debian 12. The AV1 decoder is not
# optional on this baseline: without it the only decoder GStreamer finds is
# libaom's `av1dec`, measured at 0.21 fps on a 1080p10 film against 82 fps
# through dav1d. Debian 12 ships dav1d 1.0.0, and the `dav1d-sys` crate binds
# an API that arrived in 1.3.0, so the system copy cannot be used.
#
# Static, so the finished package carries no libdav1d dependency and still
# installs on a system whose own copy is too old - which is the reason this
# image is Debian 12 in the first place.
#
# Fetched as a pinned tarball with its published checksum rather than letting
# `dav1d-sys` build it: that fallback git-clones a moving tag during `cargo
# build`, and a signed release should not reach the network in the middle of
# a compile, for source nothing verifies.
#
# --libdir=lib keeps dav1d.pc out of the multiarch directory, which
# pkg-config's default search path does not cover under /usr/local.
ARG DAV1D_VERSION=1.5.1
ARG DAV1D_SHA256=401813f1f89fa8fd4295805aa5284d9aed9bc7fc1fdbe554af4292f64cbabe21
RUN cd /tmp \
    && curl -fsSLO "https://downloads.videolan.org/pub/videolan/dav1d/${DAV1D_VERSION}/dav1d-${DAV1D_VERSION}.tar.xz" \
    && echo "${DAV1D_SHA256}  dav1d-${DAV1D_VERSION}.tar.xz" | sha256sum -c - \
    && tar xf "dav1d-${DAV1D_VERSION}.tar.xz" \
    && meson setup "dav1d-${DAV1D_VERSION}/build" "dav1d-${DAV1D_VERSION}" \
        --prefix=/usr/local --libdir=lib --buildtype=release \
        -Ddefault_library=static -Denable_tools=false -Denable_tests=false \
    && ninja -C "dav1d-${DAV1D_VERSION}/build" \
    && ninja -C "dav1d-${DAV1D_VERSION}/build" install \
    && install -Dm644 "dav1d-${DAV1D_VERSION}/COPYING" /usr/local/share/licenses/dav1d/COPYING \
    && install -Dm644 "dav1d-${DAV1D_VERSION}/doc/PATENTS" /usr/local/share/licenses/dav1d/PATENTS \
    && rm -rf /tmp/dav1d-*

# Pinned to match what CI and the other platforms build with, so a package
# differs from a developer's build only in where it was made.
ARG RUST_VERSION=1.97.1
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION}
ENV PATH="/root/.cargo/bin:${PATH}"

# Cargo's cache and build tree live outside the source tree, so a bind-mounted
# checkout is not written to and a Windows or macOS mount cannot mangle what
# lands in it. Mount volumes here to keep both between runs:
#
#   docker run ... -v tine-cargo:/cargo -v tine-target:/build ...
ENV CARGO_HOME=/cargo
ENV CARGO_TARGET_DIR=/build
VOLUME /cargo
VOLUME /build
