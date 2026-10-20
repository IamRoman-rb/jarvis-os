/* Una DLL de prueba para vino (la usa usa-dll.c): DllMain, exports y variables de TLS adentro de
 * la DLL. Se compila con /LD (DLL) y /MD (el CRT en DLL). */
#include <windows.h>

static int iniciada = 0;
__declspec(thread) int contador_tls = 5;

BOOL WINAPI DllMain(HINSTANCE h, DWORD reason, LPVOID reservado) {
    (void)h;
    (void)reservado;
    if (reason == DLL_PROCESS_ATTACH) iniciada = 1;
    return TRUE;
}

__declspec(dllexport) int sumar(int a, int b) { return a + b; }

__declspec(dllexport) const char *saludo(void) { return iniciada ? "DllMain ok" : "sin DllMain"; }

__declspec(dllexport) int tls_mas_uno(void) { return ++contador_tls; }
