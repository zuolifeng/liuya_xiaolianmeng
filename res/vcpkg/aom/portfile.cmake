# NASM is required to build AOM
vcpkg_find_acquire_program(NASM)
get_filename_component(NASM_EXE_PATH ${NASM} DIRECTORY)
vcpkg_add_to_path(${NASM_EXE_PATH})

# Perl is required to build AOM
vcpkg_find_acquire_program(PERL)
get_filename_component(PERL_PATH ${PERL} DIRECTORY)
vcpkg_add_to_path(${PERL_PATH})

if(DEFINED ENV{USE_AOM_391})
    vcpkg_from_git(
        OUT_SOURCE_PATH SOURCE_PATH
        URL "https://aomedia.googlesource.com/aom"
        REF 8ad484f8a18ed1853c094e7d3a4e023b2a92df28 # 3.9.1
        PATCHES
            aom-uninitialized-pointer.diff
            aom-avx2.diff
            aom-install.diff
    )
else()
    # 六牙象·连萌：上游 vcpkg_from_git 走 https://aomedia.googlesource.com/aom，
    # 该域名在中国大陆网络不可达（curl 长时间超时，git fetch 报 error 128）。
    # 改用 AOM 官方发布 tarball（storage.googleapis.com/aom-releases，实测可达），
    # 内容与 tag v3.12.1 (10aece4157eb79315da205f39e19bf6ab3ee30d0) 一致。
    vcpkg_download_distfile(AOM_ARCHIVE
        URLS "https://storage.googleapis.com/aom-releases/libaom-3.12.1.tar.gz"
        FILENAME "libaom-3.12.1.tar.gz"
        SHA512 27521fe1cffd89a8875552f1758de89c19a47aa1640ee20930ac420a03d964eb9ae10c4b0f55e518c37d4d59f06657aee2bfa84eedad35683648bd658e06da73
    )
    vcpkg_extract_source_archive(SOURCE_PATH
        ARCHIVE "${AOM_ARCHIVE}"
        SOURCE_BASE "3.12.1"
        PATCHES
            aom-uninitialized-pointer.diff
            # aom-avx2.diff
            # Can be dropped when https://bugs.chromium.org/p/aomedia/issues/detail?id=3029 is merged into the upstream
            aom-install.diff
    )
endif()

set(aom_target_cpu "")
set(aom_asm_compiler "")
if(VCPKG_TARGET_IS_ANDROID AND VCPKG_TARGET_ARCHITECTURE STREQUAL "arm64")
  # 六牙象·连萌: NDK 不提供 'as', 用 NDK clang wrapper 作 ASM 编译器 (自带 --target=aarch64-linux-android28)
  set(aom_asm_compiler "-DCMAKE_ASM_COMPILER=$ENV{ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android28-clang.cmd")
endif()
if(VCPKG_TARGET_IS_UWP OR (VCPKG_TARGET_IS_WINDOWS AND VCPKG_TARGET_ARCHITECTURE MATCHES "^arm"))
    # UWP + aom's assembler files result in weirdness and build failures
    # Also, disable assembly on ARM and ARM64 Windows to fix compilation issues.
    set(aom_target_cpu "-DAOM_TARGET_CPU=generic")
endif()

if(VCPKG_TARGET_ARCHITECTURE STREQUAL "arm" AND VCPKG_TARGET_IS_LINUX)
  set(aom_target_cpu "-DENABLE_NEON=OFF")
endif()

vcpkg_cmake_configure(
    SOURCE_PATH ${SOURCE_PATH}
    OPTIONS
        ${aom_target_cpu}
        ${aom_asm_compiler}
        -DENABLE_DOCS=OFF
        -DENABLE_EXAMPLES=OFF
        -DENABLE_TESTDATA=OFF
        -DENABLE_TESTS=OFF
        -DENABLE_TOOLS=OFF
)

vcpkg_cmake_install()

vcpkg_copy_pdbs()

vcpkg_fixup_pkgconfig()

if(VCPKG_TARGET_IS_WINDOWS)
  vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/lib/pkgconfig/aom.pc" " -lm" "")
  if(NOT VCPKG_BUILD_TYPE)
    vcpkg_replace_string("${CURRENT_PACKAGES_DIR}/debug/lib/pkgconfig/aom.pc" " -lm" "")
  endif()
endif()

# Move cmake configs
vcpkg_cmake_config_fixup(CONFIG_PATH lib/cmake/${PORT})

# Remove duplicate files
file(REMOVE_RECURSE ${CURRENT_PACKAGES_DIR}/debug/include
                    ${CURRENT_PACKAGES_DIR}/debug/share)

# Handle copyright
file(INSTALL ${SOURCE_PATH}/LICENSE DESTINATION ${CURRENT_PACKAGES_DIR}/share/${PORT} RENAME copyright)

vcpkg_fixup_pkgconfig()
