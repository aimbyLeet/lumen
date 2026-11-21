// Host mouse pointer drawn over the video, 1:1. The viewport is the cursor
// rectangle; `Origin` is its top-left in render-target pixels.
//   PS_Image  : straight-alpha colour pixels (blend SRC_ALPHA / INV_SRC_ALPHA)
//   PS_Invert : pixels that invert the screen (blend INV_DEST_COLOR / ZERO)

cbuffer Params : register(b0) { int2 Origin; int2 Pad; };
Texture2D<float4> Image  : register(t0);
Texture2D<float>  Invert : register(t1);

float4 VS(uint id : SV_VertexID) : SV_Position
{
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
}

float4 PS_Image(float4 pos : SV_Position) : SV_Target
{
    return Image.Load(int3(int2(pos.xy) - Origin, 0));
}

float4 PS_Invert(float4 pos : SV_Position) : SV_Target
{
    if (Invert.Load(int3(int2(pos.xy) - Origin, 0)) < 0.5)
        discard;
    return float4(1.0, 1.0, 1.0, 1.0);
}
