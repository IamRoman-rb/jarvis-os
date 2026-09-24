# Sistemas de archivos: FAT32

- El disco se divide en: sector de arranque, tablas FAT (2 copias) y datos.
- Los datos van en clusters. La FAT es una lista enlazada: para cada cluster
  dice cuál es el siguiente del mismo archivo.
- Una carpeta es un archivo con entradas de 32 bytes (nombre 8.3, fechas,
  primer cluster, tamaño). Los nombres largos usan entradas LFN extra.
