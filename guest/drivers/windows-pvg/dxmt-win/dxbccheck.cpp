// SPDX-License-Identifier: MIT
//
// dxbccheck: reads a DXBC file with DXMT's own parser (libs/DXBCParser),
// built for the Mac, and prints what it found: the chunks, the signatures,
// and every instruction with its operands. It fails if the parser refuses
// any part. Used by build.sh on the shaders make_dxbc.py writes.
//
//   dxbccheck FILE.dxbc

#include <cstdio>
#include <vector>

#include "BlobContainer.h"
#include "DXBCUtils.h"
#include "ShaderBinary.h"

using namespace microsoft;
using namespace microsoft::D3D10ShaderBinary;

static const char *opcode_name(D3D10_SB_OPCODE_TYPE op)
{
	switch (op) {
	case D3D10_SB_OPCODE_IADD: return "iadd";
	case D3D10_SB_OPCODE_CUSTOMDATA: return "customdata";
	case D3D10_SB_OPCODE_MOV: return "mov";
	case D3D10_SB_OPCODE_RET: return "ret";
	case D3D10_SB_OPCODE_DCL_INPUT_SGV: return "dcl_input_sgv";
	case D3D10_SB_OPCODE_DCL_INPUT_PS: return "dcl_input_ps";
	case D3D10_SB_OPCODE_DCL_OUTPUT: return "dcl_output";
	case D3D10_SB_OPCODE_DCL_OUTPUT_SIV: return "dcl_output_siv";
	case D3D10_SB_OPCODE_DCL_TEMPS: return "dcl_temps";
	default: return "(other)";
	}
}

static const char *type_name(D3D10_SB_OPERAND_TYPE type)
{
	switch (type) {
	case D3D10_SB_OPERAND_TYPE_TEMP: return "r";
	case D3D10_SB_OPERAND_TYPE_INPUT: return "v";
	case D3D10_SB_OPERAND_TYPE_OUTPUT: return "o";
	case D3D10_SB_OPERAND_TYPE_IMMEDIATE32: return "l";
	case D3D10_SB_OPERAND_TYPE_IMMEDIATE_CONSTANT_BUFFER: return "icb";
	default: return "?";
	}
}

static bool signature(const void *blob, bool input)
{
	CSignatureParser parser;
	HRESULT hr = input ? DXBCGetInputSignature(blob, &parser) : DXBCGetOutputSignature(blob, &parser);
	if (FAILED(hr)) {
		printf("  %s signature: refused (0x%08x)\n", input ? "input" : "output", (unsigned)hr);
		return false;
	}
	const D3D11_SIGNATURE_PARAMETER *params;
	UINT n = parser.GetParameters(&params);
	printf("  %s signature, %u elements:\n", input ? "input" : "output", n);
	for (UINT i = 0; i < n; i++)
		printf("    %s%u  system value %u  type %u  register %u  mask %x\n", params[i].SemanticName,
		       params[i].SemanticIndex, (unsigned)params[i].SystemValue,
		       (unsigned)params[i].ComponentType, params[i].Register, params[i].Mask);
	return true;
}

int main(int argc, char **argv)
{
	if (argc != 2) {
		fprintf(stderr, "usage: dxbccheck FILE.dxbc\n");
		return 2;
	}
	FILE *f = fopen(argv[1], "rb");
	if (!f) {
		perror(argv[1]);
		return 1;
	}
	std::vector<unsigned char> data;
	int c;
	while ((c = fgetc(f)) != EOF)
		data.push_back((unsigned char)c);
	fclose(f);

	CDXBCParser dxbc;
	if (FAILED(dxbc.ReadDXBC(data.data(), (UINT32)data.size()))) {
		printf("%s: the container is refused\n", argv[1]);
		return 1;
	}
	printf("%s: %zu bytes, %u chunks\n", argv[1], data.size(), dxbc.GetBlobCount());
	for (UINT32 i = 0; i < dxbc.GetBlobCount(); i++) {
		UINT fourcc = dxbc.GetBlobFourCC(i);
		printf("  chunk %c%c%c%c, %u bytes\n", fourcc & 0xff, (fourcc >> 8) & 0xff, (fourcc >> 16) & 0xff,
		       fourcc >> 24, dxbc.GetBlobSize(i));
	}
	if (!signature(data.data(), true) || !signature(data.data(), false))
		return 1;

	UINT32 at = dxbc.FindNextMatchingBlob(DXBC_GenericShader);
	if (at == DXBC_BLOB_NOT_FOUND) {
		printf("  no SHDR chunk\n");
		return 1;
	}
	const CShaderToken *tokens = (const CShaderToken *)dxbc.GetBlob(at);
	CShaderCodeParser code(tokens);
	printf("  program: %s %u.%u, %u tokens\n",
	       code.ShaderType() == D3D10_SB_VERTEX_SHADER ? "vertex" :
	       code.ShaderType() == D3D10_SB_PIXEL_SHADER ? "pixel" : "other",
	       code.ShaderMajorVersion(), code.ShaderMinorVersion(), code.ShaderLengthInTokens());
	if (code.ShaderLengthInTokens() * 4 != dxbc.GetBlobSize(at)) {
		printf("  the program's length does not match its chunk\n");
		return 1;
	}
	int count = 0;
	while (!code.EndOfShader()) {
		CInstruction ins;
		code.ParseInstruction(&ins);
		printf("    %-15s", opcode_name(ins.OpCode()));
		for (UINT i = 0; i < ins.NumOperands(); i++) {
			const COperandBase &op = ins.Operand(i);
			printf(" %s", type_name(op.OperandType()));
			if (op.OperandType() == D3D10_SB_OPERAND_TYPE_IMMEDIATE32)
				printf("(0x%08x)", op.Imm32());
			else if (op.OperandIndexType(0) == D3D10_SB_OPERAND_INDEX_RELATIVE)
				printf("[relative]");
			else
				printf("%u", op.RegIndex(0));
		}
		printf("\n");
		if (ins.OpCode() == D3D10_SB_OPCODE_RET)
			count++;
	}
	if (count != 1) {
		printf("  expected one ret\n");
		return 1;
	}
	printf("  parsed to the end\n");
	return 0;
}
