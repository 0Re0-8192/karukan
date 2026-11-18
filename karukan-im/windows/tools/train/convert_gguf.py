"""convert_hf_to_gguf.py with the BPE pre-tokenizer check skipped.

A model with a trimmed vocabulary has a tokenizer llama.cpp's converter does not recognise by hash.
karukan tokenizes outside llama.cpp (tokenizer.json beside the GGUF) and hands it ids, so
the pre-tokenizer entry in the GGUF is never used; "default" is stored.

  python convert_gguf.py LLAMA_CPP_SRC HF_DIR OUT.gguf [q8_0|f16|...]"""
import runpy, sys

src, hf, out = sys.argv[1:4]
typ = sys.argv[4] if len(sys.argv) > 4 else "q8_0"
sys.path.insert(0, src)
import conversion.base as base

base.TextModel.get_vocab_base_pre = lambda self, tokenizer: "default"
sys.argv = ["convert_hf_to_gguf.py", hf, "--outfile", out, "--outtype", typ]
runpy.run_path(src + "/convert_hf_to_gguf.py", run_name="__main__")
