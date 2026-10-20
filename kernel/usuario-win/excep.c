/* Excepciones estructuradas de Windows (SEH): __try/__except/__finally, el filtro, el desenrollado
 * y un segundo __try/__finally/__except. */
#include <stdio.h>
#include <windows.h>

static int filtro(unsigned code) {
    printf("  filtro: codigo 0x%lX\n", code);
    return EXCEPTION_EXECUTE_HANDLER;
}

int main(void) {
    /* 1. RaiseException atrapada por __except, con __finally en el medio. */
    __try {
        __try {
            printf("antes de lanzar\n");
            RaiseException(0xE0000001, 0, 0, NULL);
            printf("ESTO NO SE IMPRIME\n");
        } __finally {
            printf("  __finally corre (%s)\n", AbnormalTermination() ? "por excepcion" : "normal");
        }
    } __except (filtro(GetExceptionCode())) {
        printf("atrapada en __except\n");
    }

    /* 2. El valor del filtro se evalua en la busqueda y el __finally desenrolla. */
    int pasos = 0;
    __try {
        __try {
            RaiseException(0xE0000002, 0, 0, NULL);
        } __finally {
            pasos += 1;
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        pasos += 10;
    }
    printf("pasos: %d\n", pasos);
    printf("SEH_OK\n");
    return 0;
}
