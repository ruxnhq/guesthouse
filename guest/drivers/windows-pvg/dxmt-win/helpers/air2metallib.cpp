// SPDX-License-Identifier: MIT
//
// air2metallib: an AIR module written as LLVM assembly (helpers/*.ll) into
// a library the device takes, with airconv's own library writer, the one
// it uses for the libraries it makes from DXBC. Built on the Mac against
// the LLVM 15 and airconv built for it; no other compiler is involved.
//
// Usage: air2metallib IN.ll OUT.metallib

#include "llvm/IR/LLVMContext.h"
#include "llvm/IR/Module.h"
#include "llvm/IR/Verifier.h"
#include "llvm/IRReader/IRReader.h"
#include "llvm/Support/SourceMgr.h"
#include "llvm/Support/raw_ostream.h"
#include "metallib_writer.hpp"

int main(int argc, char **argv)
{
	if (argc != 3) {
		llvm::errs() << "usage: air2metallib IN.ll OUT.metallib\n";
		return 2;
	}
	llvm::LLVMContext context;
	llvm::SMDiagnostic error;
	std::unique_ptr<llvm::Module> module = llvm::parseIRFile(argv[1], error, context);

	if (!module) {
		error.print(argv[0], llvm::errs());
		return 1;
	}
	if (llvm::verifyModule(*module, &llvm::errs()))
		return 1;
	std::error_code code;
	llvm::raw_fd_ostream out(argv[2], code);
	if (code) {
		llvm::errs() << argv[2] << ": " << code.message() << "\n";
		return 1;
	}
	dxmt::metallib::MetallibWriter writer;
	writer.Write(*module, out);
	return 0;
}
