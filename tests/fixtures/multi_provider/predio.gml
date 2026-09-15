<?xml version="1.0" encoding="UTF-8"?>
<!--
  Fixture mínima CityGML 2.0 LOD1: um único bldg:Building com um gml:Solid
  fechado (base + topo + 4 paredes), coordenadas reais em EPSG:31983
  (SIRGAS 2000 / UTM 23S) posicionadas dentro do bbox de teste do Guará I
  usado em tests/multi_provider_smoke.rs (-15.8182,-47.9832,-15.8178,-47.9828).
  Centro do prédio em UTM (180433, 8248927) == (-15.8180, -47.9830) lat/lon,
  verificado com `cs2cs +init=epsg:4326 +to +init=epsg:31983`.
-->
<core:CityModel xmlns:core="http://www.opengis.net/citygml/2.0"
                 xmlns:bldg="http://www.opengis.net/citygml/building/2.0"
                 xmlns:gml="http://www.opengis.net/gml">
  <core:cityObjectMember>
    <bldg:Building gml:id="bldg_fixture_001">
      <bldg:function>1000</bldg:function>
      <bldg:measuredHeight uom="m">3.0</bldg:measuredHeight>
      <bldg:lod1Solid>
        <gml:Solid gml:id="solid_fixture_001">
          <gml:exterior>
            <gml:CompositeSurface>
              <gml:surfaceMember>
                <gml:Polygon gml:id="ground">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180428 8248922 1080 180438 8248922 1080 180438 8248932 1080 180428 8248932 1080 180428 8248922 1080</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
              <gml:surfaceMember>
                <gml:Polygon gml:id="roof">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180428 8248922 1083 180438 8248922 1083 180438 8248932 1083 180428 8248932 1083 180428 8248922 1083</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
              <gml:surfaceMember>
                <gml:Polygon gml:id="wall_south">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180428 8248922 1080 180438 8248922 1080 180438 8248922 1083 180428 8248922 1083 180428 8248922 1080</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
              <gml:surfaceMember>
                <gml:Polygon gml:id="wall_east">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180438 8248922 1080 180438 8248932 1080 180438 8248932 1083 180438 8248922 1083 180438 8248922 1080</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
              <gml:surfaceMember>
                <gml:Polygon gml:id="wall_north">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180438 8248932 1080 180428 8248932 1080 180428 8248932 1083 180438 8248932 1083 180438 8248932 1080</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
              <gml:surfaceMember>
                <gml:Polygon gml:id="wall_west">
                  <gml:exterior>
                    <gml:LinearRing>
                      <gml:posList srsDimension="3">180428 8248932 1080 180428 8248922 1080 180428 8248922 1083 180428 8248932 1083 180428 8248932 1080</gml:posList>
                    </gml:LinearRing>
                  </gml:exterior>
                </gml:Polygon>
              </gml:surfaceMember>
            </gml:CompositeSurface>
          </gml:exterior>
        </gml:Solid>
      </bldg:lod1Solid>
    </bldg:Building>
  </core:cityObjectMember>
</core:CityModel>
