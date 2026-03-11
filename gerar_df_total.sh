#!/bin/bash
# Configurações de caminho e coordenadas
ARNIS_BIN="/home/ubuntu/arnis/target/release/arnis"
LAT_MIN=-16.06; LAT_MAX=-15.50; LON_MIN=-48.33; LON_MAX=-47.30
LAT_STEP=0.14; LON_STEP=0.257

# 1. Cria a base e ENTRA nela
mkdir -p /home/ubuntu/arnis/mapa_completo
cd /home/ubuntu/arnis/mapa_completo

# 2. CRIA TODAS AS 16 PASTAS DE UMA VEZ (O "Pulo do Gato")
echo "Criando a estrutura de 16 quadrantes..."
mkdir -p quadrante_{0..3}_{0..3}

# 3. Loop de geração
for i in {0..3}; do
  for j in {0..3}; do
    B_LAT_MIN=$(echo "$LAT_MIN + ($i * $LAT_STEP)" | bc)
    B_LAT_MAX=$(echo "$LAT_MIN + (($i + 1) * $LAT_STEP)" | bc)
    B_LON_MIN=$(echo "$LON_MIN + ($j * $LON_STEP)" | bc)
    B_LON_MAX=$(echo "$LON_MIN + (($j + 1) * $LON_STEP)" | bc)
    
    FOLDER="quadrante_${i}_${j}"
    
    echo "----------------------------------------------------"
    echo "PROCESSANDO: $FOLDER"
    echo "----------------------------------------------------"
    
    # Roda o Arnis apontando para a pasta que JÁ EXISTE
    xvfb-run $ARNIS_BIN --terrain --path="$FOLDER" --bbox="$B_LAT_MIN,$B_LON_MIN,$B_LAT_MAX,$B_LON_MAX"
    
    echo "Pausa de 45s para respirar..."
    sleep 45
  done
done
echo "IMPÉRIO CONCLUÍDO!"
