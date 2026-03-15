use super::translate::translator_from_json;
use crate::coordinate_system::cartesian::XZBBox;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;

/// An Operator does transformation on the map, modifying Vec<ProcessedElement> and XZBBox.
/// [BESM-6 Architecture] - Camada de Mutação Espacial Pós-Projeção.
/// É aqui que os dados recém-chegados (Shapefile, GeoJSON, LiDAR) sofrem translações cartesianas
/// e ajustes finos na malha de blocos, respeitando limites geográficos do datapack (Y=4064).
/// Garantimos `Send + Sync` para viabilizar pipelines multithread em mapas massivos.
pub trait Operator: Send + Sync {
    /// Apply the operation
    fn operate(
        &self,
        elements: &mut Vec<ProcessedElement>,
        xzbbox: &mut XZBBox,
        ground: &mut Ground,
    );

    #[allow(dead_code)]
    /// Return a string describing the current specific operator
    fn repr(&self) -> String;
}

pub fn operator_from_json(config: &serde_json::Value) -> Result<Box<dyn Operator>, String> {
    let operation_str = config
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or("[ALERTA BESM-6] Operador inválido: O campo 'operation' é obrigatório no dict de operação.")?;

    let operator_config = config
        .get("config")
        .ok_or("[ALERTA BESM-6] Operador inválido: O campo 'config' é obrigatório no dict de operação.")?;

    // ?? BESM-6 Tweak: O seletor de operadores está preparado para expansão estrutural orgânica.
    // Futuros operadores de clamp de elevação (4064 blocos limit) devem ser injetados neste pipeline.
    let operator_result: Result<Box<dyn Operator>, String> = match operation_str {
        "translate" => translator_from_json(operator_config),
        _ => Err(format!("[ALERTA BESM-6] Tipo de operação não reconhecida: '{operation_str}'. O pipeline foi abortado para proteger a malha.")),
    };

    operator_result.map_err(|e| format!("Erro de formatação na configuração do Operador:\n{e}"))
}

pub fn operator_vec_from_json(list: &serde_json::Value) -> Result<Vec<Box<dyn Operator>>, String> {
    let oplist = list
        .as_array()
        .ok_or("A matriz de operadores de transformação JSON é inválida ou inexistente.".to_string())?;

    // Mapeamento funcional com rastreabilidade exata de índice
    oplist
        .iter()
        .enumerate()
        .map(|(i, v)| {
            operator_from_json(v)
                .map_err(|e| format!("Falha de sintaxe no Operador (Índice {i}):\n{e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::{XZPoint, XZVector};
    use crate::map_transformation::translate;
    use std::fs;

    // this ensures json can be correctly read into the specific operator struct
    #[test]
    fn test_read_valid_formats() {
        let opjson = serde_json::from_str(
            &fs::read_to_string("tests/map_transformation/all_valid_examples.json").unwrap(),
        )
        .unwrap();

        let ops = operator_vec_from_json(&opjson);

        assert!(ops.is_ok());

        let ops = ops.unwrap();

        // total number of operations
        assert_eq!(ops.len(), 2);

        // below tests the operators one by one by comparing repr description

        let testop = translate::VectorTranslator {
            vector: XZVector { dx: 2000, dz: 1000 },
        };
        assert_eq!(ops[0].repr(), testop.repr());

        let testop = translate::StartEndTranslator {
            start: XZPoint { x: 0, z: 0 },
            end: XZPoint { x: -1000, z: -2000 },
        };
        assert_eq!(ops[1].repr(), testop.repr());
    }

    // this ensures json format error can be handled as Err
    #[test]
    fn test_read_invalid_formats() {
        let opjson = serde_json::from_str(
            &fs::read_to_string("tests/map_transformation/invalid_example_missing_field.json")
                .unwrap(),
        )
        .unwrap();

        let ops = operator_vec_from_json(&opjson);

        assert!(ops.is_err());
    }
}