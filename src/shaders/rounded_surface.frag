// Adapted from cosmic-comp's clipped_surface.frag (GPL-3.0-only).
// Original: https://github.com/pop-os/cosmic-comp, which credits niri.
//
// Rounds the client surface by masking the sampled texture with a per-corner
// signed-distance coverage test. Unlike a paint-over ring, this clips the real
// client pixels, so no square corner can survive underneath the decoration.

precision highp float;

//_DEFINES_

uniform sampler2D tex;
uniform float alpha;
varying vec2 v_coords;

// Destination rectangle of the element, in framebuffer pixels.
uniform vec2 geo_origin;
uniform vec2 geo_size;
// Per-corner radius, ordered top-left, top-right, bottom-right, bottom-left.
uniform vec4 corner_radius;

float rounding_alpha(vec2 coords, vec2 size, vec4 radius) {
    vec2 center;
    float r;

    if (coords.x < radius.x && coords.y < radius.x) {
        r = radius.x;
        center = vec2(r, r);
    } else if (size.x - radius.y < coords.x && coords.y < radius.y) {
        r = radius.y;
        center = vec2(size.x - r, r);
    } else if (size.x - radius.z < coords.x && size.y - radius.z < coords.y) {
        r = radius.z;
        center = vec2(size.x - r, size.y - r);
    } else if (coords.x < radius.w && size.y - radius.w < coords.y) {
        r = radius.w;
        center = vec2(r, size.y - r);
    } else {
        return 1.0;
    }

    float dist = distance(coords, center);
    // Half-pixel antialias band, same shape as cosmic's smoothstep.
    return 1.0 - smoothstep(r - 0.5, r + 0.5, dist);
}

void main() {
    if (alpha <= 0.0) {
        discard;
    }

    vec4 color = texture2D(tex, v_coords);
    if (color.a <= 0.0) {
        discard;
    }

    vec2 coords = gl_FragCoord.xy - geo_origin;
    if (coords.x < 0.0 || geo_size.x < coords.x
            || coords.y < 0.0 || geo_size.y < coords.y) {
        color = vec4(0.0);
    } else {
        color *= rounding_alpha(coords, geo_size, corner_radius);
    }

    if (color.a <= 0.0) {
        discard;
    }

    gl_FragColor = color * alpha;
}
