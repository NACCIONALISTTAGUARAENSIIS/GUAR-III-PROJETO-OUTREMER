//! Auditoria de proveniência: pra cada `Feature` processada durante uma
//! geração, registra qual(is) provedor(es) a forneceram e qual(is) módulo(s)
//! de `element_processing` de fato colocaram blocos por causa dela — inclusive
//! os casos em que MAIS DE UM módulo trata a mesma feature (ex.: uma via de
//! energia passa por `power::generate_power` E por
//! `generate_underground_infrastructure` na mesma chamada de
//! `dispatch_element` — uma "mistura" real do pipeline atual, não hipotética).
//!
//! 🚨 BESM-6: por que isto existe. Pedido explícito do usuário: auditabilidade
//! — saber o que foi gerado por qual provider/módulo sem precisar consultar o
//! código toda vez. Investigação prévia (nesta sessão) encontrou um problema
//! real: `Feature::source`/`semantic_group`/`priority` são DESCARTADOS assim
//! que `Feature::into_processed_element()` roda (só `id`/`geometry`/
//! `attributes` sobrevivem) — então, sem este módulo, o dispatcher genérico
//! (`data_processing::dispatch_element`, que roteia ~820 chamadas de bloco
//! espalhadas por 21 módulos de `element_processing` conforme as tags OSM)
//! não tem NENHUMA forma de saber de onde um elemento veio.
//!
//! **Granularidade escolhida deliberadamente (por feature/estrutura, não por
//! bloco individual):** rastrear por bloco exigiria instrumentar todos os
//! ~820 pontos de chamada de `WorldEditor::set_block*` em 21 módulos — uma
//! mudança muito mais invasiva e arriscada. Por feature aproveita os campos
//! que já existem (`Feature::id`/`source`/`semantic_group`) e cobre a
//! pergunta real do usuário ("o que gerou isto aqui") sem reescrever a
//! geração inteira. `ProvenanceLedger::register_origin` guarda a origem de
//! CADA `Feature` antes dela virar `ProcessedElement` (antes da informação se
//! perder); `record_dispatch`/`record_direct` registram, depois, qual(is)
//! módulo(s) realmente a processaram.
//!
//! **Cobertura:** só o pipeline principal (`data_processing::generate_world_with_options`,
//! chamado por `main::run_generation_pipeline` — o caminho real de
//! `pincelism <bbox>`). O modo HUD interativo (`generate_region_from_global`,
//! usado só sem a feature `gui`, streaming região-por-região) recebe um
//! ledger novo e descartado por chamada — não teria como acumular um
//! relatório coerente entre chamadas independentes sem replanejar esse modo
//! por completo. Documentado aqui como limitação real, não escondida.

use crate::providers::Feature;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// O que se sabia sobre uma `Feature` ANTES dela virar `ProcessedElement` —
/// exatamente os dois campos que `into_processed_element` descarta.
#[derive(Clone)]
struct FeatureOrigin {
    source: String,
    semantic_group: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProvenanceRecord {
    #[serde(rename = "featureId")]
    pub feature_id: u64,
    pub source: String,
    #[serde(rename = "semanticGroup")]
    pub semantic_group: String,
    /// Quase sempre 1 módulo; mais de 1 é uma mistura real (ver comentário de
    /// módulo) — nunca vazio (registros com zero módulos não são gravados).
    pub modules: Vec<String>,
}

#[derive(Serialize)]
struct ProvenanceSummary {
    #[serde(rename = "totalFeaturesRegistered")]
    total_features_registered: usize,
    #[serde(rename = "totalFeaturesDispatched")]
    total_features_dispatched: usize,
    #[serde(rename = "byProvider")]
    by_provider: HashMap<String, usize>,
    #[serde(rename = "byModule")]
    by_module: HashMap<String, usize>,
    #[serde(rename = "byProviderAndModule")]
    by_provider_and_module: HashMap<String, usize>,
    #[serde(rename = "bySemanticGroup")]
    by_semantic_group: HashMap<String, usize>,
    /// Features processadas por MAIS DE UM módulo na mesma geração — a
    /// "mistura" que a auditoria existe pra revelar sem ler código.
    #[serde(rename = "mixedModuleFeatures")]
    mixed_module_features: Vec<ProvenanceRecord>,
}

#[derive(Default)]
pub struct ProvenanceLedger {
    origins: HashMap<u64, FeatureOrigin>,
    records: Vec<ProvenanceRecord>,
}

impl ProvenanceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra a origem de uma `Feature` — chamar ANTES de
    /// `feature.into_processed_element()` / `feature.clone().into_processed_element()`,
    /// nunca depois (a informação já teria sumido).
    pub fn register_origin(&mut self, feature: &Feature) {
        self.origins.insert(
            feature.id,
            FeatureOrigin {
                source: feature.source.clone(),
                semantic_group: format!("{:?}", feature.semantic_group),
            },
        );
    }

    /// Registra que `feature_id` foi tratado por `modules` — usado em
    /// `dispatch_element`, depois que se sabe (pelas tags) qual(is) módulo(s)
    /// rodaram. Não faz nada se `modules` estiver vazio (elemento cujas tags
    /// não bateram com nenhum branch conhecido) ou se `feature_id` não tiver
    /// origem registrada (não deveria acontecer para o elemento de TOPO
    /// despachado — só sub-elementos sintéticos internos, que nunca chegam
    /// aqui diretamente, ficariam sem origem).
    pub fn record_dispatch(&mut self, feature_id: u64, modules: Vec<String>) {
        if modules.is_empty() {
            return;
        }
        if let Some(origin) = self.origins.get(&feature_id) {
            self.records.push(ProvenanceRecord {
                feature_id,
                source: origin.source.clone(),
                semantic_group: origin.semantic_group.clone(),
                modules,
            });
        }
    }

    /// Registro direto pra features de provedor que nunca passam por
    /// `dispatch_element` (CAESB/fotogrametria/advertising — despachadas
    /// direto pro motor especializado em `data_processing.rs`, sem conversão
    /// pra `ProcessedElement`). Não depende de `register_origin` — a
    /// `Feature` completa ainda está disponível na hora da chamada.
    pub fn record_direct(&mut self, feature: &Feature, module: &str) {
        self.records.push(ProvenanceRecord {
            feature_id: feature.id,
            source: feature.source.clone(),
            semantic_group: format!("{:?}", feature.semantic_group),
            modules: vec![module.to_string()],
        });
    }

    fn build_summary(&self) -> ProvenanceSummary {
        let mut by_provider: HashMap<String, usize> = HashMap::new();
        let mut by_module: HashMap<String, usize> = HashMap::new();
        let mut by_provider_and_module: HashMap<String, usize> = HashMap::new();
        let mut by_semantic_group: HashMap<String, usize> = HashMap::new();
        let mut mixed_module_features = Vec::new();

        for record in &self.records {
            *by_provider.entry(record.source.clone()).or_insert(0) += 1;
            *by_semantic_group
                .entry(record.semantic_group.clone())
                .or_insert(0) += 1;
            for module in &record.modules {
                *by_module.entry(module.clone()).or_insert(0) += 1;
                *by_provider_and_module
                    .entry(format!("{} -> {}", record.source, module))
                    .or_insert(0) += 1;
            }
            if record.modules.len() > 1 {
                mixed_module_features.push(record.clone());
            }
        }

        ProvenanceSummary {
            total_features_registered: self.origins.len(),
            total_features_dispatched: self.records.len(),
            by_provider,
            by_module,
            by_provider_and_module,
            by_semantic_group,
            mixed_module_features,
        }
    }

    /// Escreve `provenance.ndjson` (um registro JSON por linha, um por
    /// feature processada) e `provenance_summary.json` (agregados — por
    /// provider, por módulo, por par provider×módulo, por grupo semântico, e
    /// a lista de features "mistas") em `world_dir`. Chamado no fim de
    /// `generate_world_with_options`, mesmo diretório onde `metadata.json` já
    /// é gravado.
    pub fn write_reports(&self, world_dir: &Path) -> std::io::Result<()> {
        let ndjson_path = world_dir.join("provenance.ndjson");
        let mut ndjson = String::new();
        for record in &self.records {
            // Cada registro serializa isoladamente — um erro de serialização
            // num único registro (não deveria acontecer, são tipos simples)
            // não derruba o relatório inteiro.
            if let Ok(line) = serde_json::to_string(record) {
                ndjson.push_str(&line);
                ndjson.push('\n');
            }
        }
        std::fs::write(&ndjson_path, ndjson)?;

        let summary = self.build_summary();
        let summary_json =
            serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(world_dir.join("provenance_summary.json"), summary_json)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Feature, GeometryType, SemanticGroup};

    fn make_feature(id: u64, source: &str, group: SemanticGroup) -> Feature {
        Feature::new(
            id,
            group,
            HashMap::new(),
            GeometryType::Point(crate::coordinate_system::cartesian::XZPoint { x: 0, z: 0 }),
            source.to_string(),
            10,
        )
    }

    #[test]
    fn record_dispatch_ignores_unknown_feature_id() {
        let mut ledger = ProvenanceLedger::new();
        // Nunca chamou register_origin para o id 42.
        ledger.record_dispatch(42, vec!["buildings".to_string()]);
        assert_eq!(ledger.records.len(), 0);
    }

    #[test]
    fn record_dispatch_ignores_empty_modules() {
        let mut ledger = ProvenanceLedger::new();
        let feature = make_feature(1, "osm", SemanticGroup::Building);
        ledger.register_origin(&feature);
        ledger.record_dispatch(1, vec![]);
        assert_eq!(ledger.records.len(), 0);
    }

    #[test]
    fn record_dispatch_recovers_origin_registered_earlier() {
        let mut ledger = ProvenanceLedger::new();
        let feature = make_feature(7, "gdf_shapefile", SemanticGroup::Building);
        ledger.register_origin(&feature);
        ledger.record_dispatch(7, vec!["buildings".to_string()]);

        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].source, "gdf_shapefile");
        assert_eq!(ledger.records[0].modules, vec!["buildings".to_string()]);
    }

    /// Regressão do caso real de "mistura" já existente no pipeline: uma way
    /// com `power`/`man_made` é processada tanto pelo branch principal quanto
    /// por `generate_underground_infrastructure` na mesma chamada de
    /// `dispatch_element` — precisa aparecer como 1 registro com 2 módulos,
    /// não 2 registros separados.
    #[test]
    fn a_feature_dispatched_to_two_modules_is_one_record_with_two_modules() {
        let mut ledger = ProvenanceLedger::new();
        let feature = make_feature(9, "caesb_wfs", SemanticGroup::Power);
        ledger.register_origin(&feature);
        ledger.record_dispatch(
            9,
            vec![
                "power".to_string(),
                "underground_infrastructure".to_string(),
            ],
        );

        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].modules.len(), 2);

        let summary = ledger.build_summary();
        assert_eq!(summary.mixed_module_features.len(), 1);
        assert_eq!(summary.by_module.get("power"), Some(&1));
        assert_eq!(
            summary.by_module.get("underground_infrastructure"),
            Some(&1)
        );
    }

    #[test]
    fn summary_aggregates_by_provider_and_module_pair() {
        let mut ledger = ProvenanceLedger::new();
        let f1 = make_feature(1, "osm", SemanticGroup::Building);
        let f2 = make_feature(2, "osm", SemanticGroup::Highway);
        ledger.register_origin(&f1);
        ledger.register_origin(&f2);
        ledger.record_dispatch(1, vec!["buildings".to_string()]);
        ledger.record_dispatch(2, vec!["highways".to_string()]);

        let summary = ledger.build_summary();
        assert_eq!(summary.by_provider.get("osm"), Some(&2));
        assert_eq!(
            summary.by_provider_and_module.get("osm -> buildings"),
            Some(&1)
        );
        assert_eq!(
            summary.by_provider_and_module.get("osm -> highways"),
            Some(&1)
        );
        assert_eq!(summary.total_features_registered, 2);
        assert_eq!(summary.total_features_dispatched, 2);
    }

    #[test]
    fn record_direct_does_not_need_prior_registration() {
        let mut ledger = ProvenanceLedger::new();
        let feature = make_feature(5, "CAESB_WFS_Live", SemanticGroup::Sanitation);
        ledger.record_direct(&feature, "man_made::generate_from_provider_feature");
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].source, "CAESB_WFS_Live");
    }

    #[test]
    fn write_reports_produces_valid_ndjson_and_summary() {
        let mut ledger = ProvenanceLedger::new();
        let feature = make_feature(3, "osm", SemanticGroup::Building);
        ledger.register_origin(&feature);
        ledger.record_dispatch(3, vec!["buildings".to_string()]);

        let tmp = tempfile::tempdir().expect("tmp dir");
        ledger.write_reports(tmp.path()).expect("write reports");

        let ndjson = std::fs::read_to_string(tmp.path().join("provenance.ndjson")).unwrap();
        let parsed: ProvenanceRecordDe =
            serde_json::from_str(ndjson.lines().next().unwrap()).unwrap();
        assert_eq!(parsed.feature_id, 3);

        let summary_text =
            std::fs::read_to_string(tmp.path().join("provenance_summary.json")).unwrap();
        assert!(summary_text.contains("totalFeaturesDispatched"));
    }

    // Espelha só os campos que o teste acima precisa ler de volta — evita
    // depender de `ProvenanceRecord: Deserialize` (não precisamos disso em
    // produção, só neste teste).
    #[derive(serde::Deserialize)]
    struct ProvenanceRecordDe {
        #[serde(rename = "featureId")]
        feature_id: u64,
    }
}
