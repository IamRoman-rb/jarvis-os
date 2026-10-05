/* Programa de Windows de prueba (consola). Lo compila `cargo xtask` con el compilador de Visual
 * Studio, igual que se compila cualquier programa de Windows: con el CRT en una DLL (/MD, lo más
 * común) y con el CRT adentro (/MT). Ejercita lo que usa un programa de consola típico: printf,
 * argumentos, memoria, archivos, la entrada, matemática y la hora. */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>

static int contador = 0;
static void al_salir(void) { printf("atexit: chau (%d)\n", contador); }

int main(int argc, char **argv) {
    atexit(al_salir);
    printf("Hola desde Windows! argc=%d\n", argc);
    for (int i = 1; i < argc; i++) printf("  argv[%d] = %s\n", i, argv[i]);

    /* Memoria: malloc, realloc, calloc, free. */
    char *buf = malloc(16);
    strcpy(buf, "memoria");
    buf = realloc(buf, 4096);
    strcat(buf, " ok");
    int *ceros = calloc(100, sizeof(int));
    int suma = 0;
    for (int i = 0; i < 100; i++) suma += ceros[i];
    printf("%s, calloc suma %d\n", buf, suma);
    free(ceros);
    free(buf);

    /* printf con formatos. */
    printf("enteros: %d %5d|%-5d|%05d %u %x %X %lld\n", -42, 7, 7, 7, 3000000000u, 255, 255,
           1234567890123LL);
    printf("textos: [%s] [%10s] [%-6s] %c%c\n", "jarvis", "der", "izq", 'O', 'K');
    printf("reales: %.2f %8.3f %e %g\n", 3.14159, -2.5, 12345.678, 0.0001);
    printf("raiz de 2 = %.6f, seno(1) = %.4f, 2^10 = %.0f\n", sqrt(2.0), sin(1.0), pow(2, 10));

    /* Archivos. */
    FILE *f = fopen("prueba-win.txt", "w");
    if (!f) { printf("no se pudo crear el archivo\n"); return 1; }
    fprintf(f, "linea 1\nlinea %d\n", 2);
    fclose(f);
    f = fopen("prueba-win.txt", "r");
    char linea[64];
    int n = 0;
    while (fgets(linea, sizeof linea, f)) n++;
    fclose(f);
    printf("archivo: %d lineas\n", n);

    /* La API de Windows directa. */
    char nombre[MAX_PATH];
    DWORD largo = GetModuleFileNameA(NULL, nombre, MAX_PATH);
    printf("GetModuleFileNameA: %s\n", largo ? "ok" : "fallo");
    SYSTEMTIME st;
    GetLocalTime(&st);
    printf("hora valida: %s\n", st.wYear >= 2024 ? "si" : "no");
    printf("tick: %s\n", GetTickCount64() > 0 ? "ok" : "fallo");
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    DWORD escrito;
    WriteConsoleA(out, "WriteConsoleA ok\n", 17, &escrito, NULL);
    time_t t = time(NULL);
    printf("time: %s\n", t > 1700000000 ? "ok" : "fallo");

    /* La entrada: una línea de la Terminal. */
    printf("Como te llamas? ");
    fflush(stdout);
    char quien[64] = "";
    if (fgets(quien, sizeof quien, stdin)) {
        quien[strcspn(quien, "\r\n")] = 0;
        printf("Hola, %s!\n", quien);
    }
    contador = 3;
    return 0;
}
