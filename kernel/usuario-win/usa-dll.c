/* Programa de prueba para vino: usa una DLL propia (saludo.dll), por import y con LoadLibrary. */
#include <stdio.h>
#include <string.h>
#include <windows.h>

__declspec(dllimport) int sumar(int a, int b);
__declspec(dllimport) const char *saludo(void);
__declspec(dllimport) int tls_mas_uno(void);

int main(void) {
    printf("dll: %s, 2+3=%d, tls=%d\n", saludo(), sumar(2, 3), tls_mas_uno());

    HMODULE m = LoadLibraryA("saludo.dll");
    int (*f)(int, int) = m ? (int (*)(int, int))GetProcAddress(m, "sumar") : NULL;
    printf("LoadLibrary: %s, GetProcAddress: %d\n", m ? "ok" : "fallo", f ? f(40, 2) : -1);

    char ruta[MAX_PATH] = "";
    GetModuleFileNameA(m, ruta, MAX_PATH);
    _strlwr(ruta);
    printf("ruta de la DLL: %s\n", strstr(ruta, "saludo.dll") ? "ok" : ruta);

    HMODULE otra = LoadLibraryA("no-existe.dll");
    printf("no-existe.dll: %s\n", otra ? "cargada?" : "NULL ok");
    return 0;
}
