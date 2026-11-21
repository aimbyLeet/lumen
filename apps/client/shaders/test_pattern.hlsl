// Full-window triangle; pixel (x, y) gets exact 10-bit codes:
//   R = (x mod 1024), G = (y mod 1024), B = 1023 - (x mod 1024)
float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}

float4 ps_main(float4 pos : SV_Position) : SV_Target {
    uint x = ((uint)pos.x) & 1023u;
    uint y = ((uint)pos.y) & 1023u;
    return float4(x / 1023.0, y / 1023.0, (1023u - x) / 1023.0, 1.0);
}
