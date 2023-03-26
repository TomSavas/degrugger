#include "funcs.h"

float silly_pow(float a, int b) {
    float pow = 1.0;

    for (int i = 0; i < b; i++) {
        pow *= a;
    }

    return pow;
}
