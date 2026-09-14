#!/bin/bash
# IMPÉRIO ROGACIONISTA - FOCO URBANO (100 QUADRANTES) | ESCALA 1.5
ARNIS_BIN="/home/ubuntu/arnis/target/release/arnis"
MAPA_DIR="/home/ubuntu/arnis/mapa_completo"
CACHE_DIR="/home/ubuntu/arnis/mapa_completo/arnis-tile-cache"

# BBox Refinado (Coração do DF e Cinturão Verde)
LAT_MIN=-16.10; LAT_MAX=-15.55
LON_MIN=-48.15; LON_MAX=-47.75

LAT_STEP=$(echo "($LAT_MAX - $LAT_MIN) / 10" | bc -l)
LON_STEP=$(echo "($LON_MAX - $LON_MIN) / 10" | bc -l)

mkdir -p "$MAPA_DIR"
cd "$MAPA_DIR"

COUNT=0

for i in {0..9}; do
  for j in {0..9}; do
    B_LAT_MIN=$(echo "$LAT_MIN + ($i * $LAT_STEP)" | bc -l)
    B_LAT_MAX=$(echo "$LAT_MIN + (($i + 1) * $LAT_STEP)" | bc -l)
    B_LON_MIN=$(echo "$LON_MIN + ($j * $LON_STEP)" | bc -l)
    B_LON_MAX=$(echo "$LON_MIN + (($j + 1) * $LON_STEP)" | bc -l)
    
    FOLDER="quadr_10x10_${i}_${j}"
    mkdir -p "$FOLDER"
    COUNT=$((COUNT + 1))
    
    echo "----------------------------------------------------"
    echo "QUADRANTE [$i,$j] ($COUNT/100) | RIGOR MATEMÁTICO 1.5"
    echo "----------------------------------------------------"
    
    # Execução do Arnis v2.5.0 com a escala perfeita
    xvfb-run $ARNIS_BIN --terrain --output-dir "$FOLDER" --bbox "$B_LAT_MIN,$B_LON_MIN,$B_LAT_MAX,$B_LON_MAX" --scale 1.5
    
    # Limpeza de cache TOTAL a cada quadrante (Proteção do SSD)
    echo "[INFO] Limpeza de segurança: esvaziando cache..."
    rm -rf "$CACHE_DIR"/*
    
    echo "Pausa de 60s (Resfriamento Overpass)..."
    sleep 60
  done
done
echo "BRASÍLIA 1.5 CONCLUÍDA!"
