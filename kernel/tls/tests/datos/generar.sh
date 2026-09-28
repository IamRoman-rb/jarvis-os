#!/bin/sh
# Certificados de prueba de los tests de TLS (no se usan en ningún otro lado). Vigencia: 100 años.
# Tres autoridades (ECDSA P-256, ECDSA P-384, RSA 2048), un servidor por cada una con el nombre
# prueba.jarvis, una autoridad "extraña" que no está entre las raíces de los tests y una
# "impostora" con el mismo nombre que ca-ec256.
set -e
# En Git Bash (Windows), que "/CN=..." no se convierta en una ruta.
export MSYS_NO_PATHCONV=1
cd "$(dirname "$0")"
DIAS=36500
ca() { # nombre, opciones de clave, hash[, nombre en el certificado]
  openssl genpkey $2 -out "$1.key" 2>/dev/null
  openssl req -x509 -new -key "$1.key" -"$3" -days $DIAS -subj "/CN=JARVIS prueba ${4:-$1}" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" -out "$1.pem"
}
srv() { # nombre, opciones de clave, autoridad, hash[, nombre alternativo]
  openssl genpkey $2 -out "$1.key" 2>/dev/null
  SAN=${5:-DNS:prueba.jarvis}
  openssl req -new -key "$1.key" -subj "/CN=${SAN#*:}" -out "$1.csr"
  printf 'subjectAltName=%s\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n' "$SAN" > ext.cnf
  openssl x509 -req -in "$1.csr" -CA "$3.pem" -CAkey "$3.key" -CAcreateserial -days $DIAS -"$4" \
    -extfile ext.cnf -out "$1.pem" 2>/dev/null
  rm -f "$1.csr" ext.cnf
}
EC256="-algorithm EC -pkeyopt ec_paramgen_curve:P-256"
EC384="-algorithm EC -pkeyopt ec_paramgen_curve:P-384"
RSA="-algorithm RSA -pkeyopt rsa_keygen_bits:2048"
ca ca-ec256 "$EC256" sha256
ca ca-ec384 "$EC384" sha384
ca ca-rsa "$RSA" sha256
ca ca-extrana "$EC256" sha256
# Se hace pasar por ca-ec256 (mismo nombre, otra clave): solo la firma la delata.
ca ca-impostora "$EC256" sha256 ca-ec256
srv srv-ec256 "$EC256" ca-ec256 sha256
srv srv-ec384 "$EC384" ca-ec384 sha384
srv srv-rsa "$RSA" ca-rsa sha384
srv srv-extrano "$EC256" ca-extrana sha256
srv srv-impostor "$EC256" ca-impostora sha256
# Para los tests de la red (jarvis-net): HTTPS a 127.0.0.1 por la placa loopback.
srv srv-ip "$EC256" ca-ec256 sha256 IP:127.0.0.1
rm -f ./*.srl
