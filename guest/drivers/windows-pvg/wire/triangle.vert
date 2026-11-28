#version 450 // SPDX-License-Identifier: MIT
// The triangle of kk-triangle: three corners from the vertex's index, no
// vertex buffer, a colour for each corner.
layout(location = 0) out vec3 colour;

const vec2 corners[3] = vec2[](vec2(0.0, -0.75), vec2(0.75, 0.75), vec2(-0.75, 0.75));
const vec3 colours[3] = vec3[](vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0));

void main()
{
	gl_Position = vec4(corners[gl_VertexIndex], 0.0, 1.0);
	colour = colours[gl_VertexIndex];
}
