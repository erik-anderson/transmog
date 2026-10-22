#include <transmog/transmog.h>

// Also verify that the installed compatibility header compiles as C.
uint32_t transmog_c_header_version(void) { return transmog_sdk_abi_version(); }
