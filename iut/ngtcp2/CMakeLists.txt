cmake_minimum_required(VERSION 3.16)
project(nesquic_ngtcp2 C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# ngtcp2 is linked statically; BoringSSL must be linked dynamically so the
# LD_PRELOAD monitor can interpose its crypto calls (see docs/CLI.md).
set(NGTCP2_PREFIX "/usr/local" CACHE PATH "ngtcp2 install prefix")
set(BORINGSSL_PREFIX "/opt/boringssl" CACHE PATH "BoringSSL install prefix")

add_executable(nesquic-ngtcp2
    src/main.c
    src/common.c
    src/client.c
    src/server.c
)

target_compile_options(nesquic-ngtcp2 PRIVATE -Wall -Wextra)
target_include_directories(nesquic-ngtcp2 PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/../c-common
    ${NGTCP2_PREFIX}/include
    ${BORINGSSL_PREFIX}/include
)
target_link_libraries(nesquic-ngtcp2 PRIVATE
    ${NGTCP2_PREFIX}/lib/libngtcp2_crypto_boringssl.a
    ${NGTCP2_PREFIX}/lib/libngtcp2.a
    ${BORINGSSL_PREFIX}/lib/libssl.so
    ${BORINGSSL_PREFIX}/lib/libcrypto.so
)
set_target_properties(nesquic-ngtcp2 PROPERTIES
    BUILD_RPATH ${BORINGSSL_PREFIX}/lib
)

install(TARGETS nesquic-ngtcp2 RUNTIME DESTINATION bin)
