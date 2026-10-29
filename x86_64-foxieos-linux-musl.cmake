# ChatGPT generated

# foxie-x86_64-musl.cmake

# Target platform
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR x86_64)

# Clang cross-compilation target
set(CMAKE_C_COMPILER clang)
set(CMAKE_C_COMPILER_TARGET x86_64-unknown-linux-musl)

set(CMAKE_CXX_COMPILER clang++)
set(CMAKE_CXX_COMPILER_TARGET x86_64-unknown-linux-musl)

# LLVM tools
set(CMAKE_AR llvm-ar)
set(CMAKE_RANLIB llvm-ranlib)
set(CMAKE_STRIP llvm-strip)

# Require paths supplied by the build command.
if(NOT DEFINED FOXIE_ROOTFS OR FOXIE_ROOTFS STREQUAL "")
    message(FATAL_ERROR "Pass -DFOXIE_ROOTFS=/path/to/rootfs")
endif()

if(NOT DEFINED FOXIE_DEV_SYSROOT OR FOXIE_DEV_SYSROOT STREQUAL "")
    message(FATAL_ERROR "Pass -DFOXIE_DEV_SYSROOT=/path/to/dev_sysroot")
endif()

# Target root filesystem
set(CMAKE_SYSROOT "${FOXIE_ROOTFS}")

# Development headers
set(FOXIE_INCLUDE_DIR "${FOXIE_DEV_SYSROOT}/system/include")

# Make custom paths available during compiler checks too
list(APPEND CMAKE_TRY_COMPILE_PLATFORM_VARIABLES
    FOXIE_ROOTFS
    FOXIE_DEV_SYSROOT
)

set(CMAKE_ASM_FLAGS_INIT "-isystem ${FOXIE_INCLUDE_DIR} -I${FOXIE_INCLUDE_DIR}")
set(CMAKE_C_FLAGS_INIT "-isystem ${FOXIE_INCLUDE_DIR} -I${FOXIE_INCLUDE_DIR}")
set(CMAKE_CXX_FLAGS_INIT "-isystem ${FOXIE_INCLUDE_DIR}/c++/v1 -isystem ${FOXIE_INCLUDE_DIR} -stdlib=libc++ -I${FOXIE_INCLUDE_DIR}/c++/v1 -I${FOXIE_INCLUDE_DIR}")

# Linker flags
set(FOXIE_LINK_FLAGS
    "--rtlib=compiler-rt"
    "-stdlib=libc++"
    "-B${CMAKE_SYSROOT}/system/lib"
    "-L${CMAKE_SYSROOT}/system/lib"
    "-Wl,-rpath,/system/lib"
    "-Wl,-dynamic-linker=/system/lib/ld-musl-x86_64.so.1"
)
string(JOIN " " FOXIE_LINK_FLAGS_STR ${FOXIE_LINK_FLAGS})

set(CMAKE_EXE_LINKER_FLAGS_INIT "${FOXIE_LINK_FLAGS_STR}")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "${FOXIE_LINK_FLAGS_STR}")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "${FOXIE_LINK_FLAGS_STR}")

# Search target dependencies, not host libraries
set(CMAKE_FIND_ROOT_PATH
    "${CMAKE_SYSROOT}"
    "${FOXIE_DEV_SYSROOT}"
)

set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)

# Target installation prefix for dependency discovery
set(CMAKE_PREFIX_PATH "/system")