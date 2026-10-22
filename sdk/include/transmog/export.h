#pragma once

#if defined(_WIN32)
#if defined(TRANSMOG_SDK_BUILD)
#define TRANSMOG_API __declspec(dllexport)
#else
#define TRANSMOG_API __declspec(dllimport)
#endif
#else
#define TRANSMOG_API __attribute__((visibility("default")))
#endif
