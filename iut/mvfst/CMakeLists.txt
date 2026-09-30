cmake_minimum_required(VERSION 3.16)
project(nesquic_mvfst CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# mvfst, fizz and folly come from getdeps (see docker/Dockerfile.mvfst); pass
# their install prefixes via CMAKE_PREFIX_PATH.
find_package(mvfst CONFIG REQUIRED)

add_executable(nesquic-mvfst
    src/main.cpp
    src/client.cpp
    src/server.cpp
)

target_compile_options(nesquic-mvfst PRIVATE -Wall -Wextra -Wno-unused-parameter)
target_include_directories(nesquic-mvfst PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../c-common)
# mvfst_LIBRARIES also names targets that are no longer exported; link the
# ones that exist.
foreach(lib IN LISTS mvfst_LIBRARIES)
    if(TARGET ${lib})
        target_link_libraries(nesquic-mvfst PRIVATE ${lib})
    endif()
endforeach()

install(TARGETS nesquic-mvfst RUNTIME DESTINATION bin)
