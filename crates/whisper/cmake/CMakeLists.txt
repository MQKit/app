# whisper.cpp as a library inside another project (not "standalone": no examples,
# tests or JavaScript bindings).
cmake_minimum_required(VERSION 3.14)
project(convertor_whisper C CXX)
add_subdirectory(../whisper.cpp whisper.cpp)
