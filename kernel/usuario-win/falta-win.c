/* Programa de Windows de prueba: usa una función que vino no tiene (GetSystemPowerStatus, de
 * kernel32). Tiene que imprimir "antes" y que vino diga cuál falta, en vez de colgarse. */
#include <stdio.h>
#include <windows.h>

int main(void) {
    printf("antes\n");
    fflush(stdout);
    SYSTEM_POWER_STATUS s;
    GetSystemPowerStatus(&s);
    printf("despues: no tendria que llegar\n");
    return 0;
}
