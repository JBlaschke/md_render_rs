# Cross-check helper for the tarfs crate: drives TarFS.jl (the reference
# implementation) on the fixture tree shared with tests/common/mod.rs.
#
#   julia --project=ENV tarfs_jl.jl OUTDIR [OUR_TAR OUR_EXTENDED_TAR]
#
# Writes into OUTDIR:
#   tarfs_jl.tar            TarFS.write_tarball of TREE   (the golden tar bytes)
#   tarfs_jl.tar.gz         TarFS.write_tarball_gz of TREE
#   tarfs_jl_viatar.tar.gz  TarFS.write_tarball_gz_viatar of TREE + LONG_PATH
#   tar_create.tar.gz       Tar.create of TREE materialized on disk
# Given OUR_TAR / OUR_EXTENDED_TAR (written by the Rust crate), it also validates
# them strictly with Tar.list and Tar.extract, compares their contents with TREE,
# and requires OUR_TAR to equal tarfs_jl.tar byte for byte.
#
# ENV is a Julia environment with TarFS.jl developed into it, e.g.
#   JULIA_PKG_OFFLINE=true julia --project=ENV -e 'using Pkg; Pkg.develop(path="path/to/TarFS.jl")'
# Regenerate the committed golden file with:
#   julia --project=ENV tarfs_jl.jl OUT && cp OUT/tarfs_jl.tar .

using TarFS, Tar
# TarFS.jl's own dependencies, reached through it so ENV needs nothing else.
using TarFS: CodecZlib, TranscodingStreams
using .CodecZlib: GzipCompressorStream

const NAME_100   = "m"^100
const LONG_SPLIT = "p" * "x"^60 * "/" * "y"^60 * ".txt"
const MULTIBYTE  = "α"^40 * "/" * "β"^40 * ".txt"
const LONG_PATH  = "L/" * join(fill("s"^40, 7), "/") * "/leaf.txt"   # unsplittable

const TREE = Dict{String, Vector{UInt8}}(
    "a.txt"             => codeunits("hello"),
    "data/p175.csv"     => codeunits("x,y\n1,2\n3,4\n"),
    "data/empty.bin"    => UInt8[],
    "bytes.bin"         => UInt8.(0:255),
    "データ/日本語.txt"    => codeunits("unicode ✓ content"),
    NAME_100            => codeunits("at the boundary"),
    LONG_SPLIT          => codeunits("split me"),
    MULTIBYTE           => codeunits("multibyte split"),
    "big.bin"           => UInt8[(i * 31 + 7) % 256 for i in 0:(3 * 512 + 16)],
    "exact512.bin"      => fill(UInt8('z'), 512),
)

function fill_fs!(fs, tree)
    for (p, c) in tree
        TarFS.writefile(fs, p, c)
    end
    fs
end

function check_extracted(tarfile, tree)
    headers = Tar.list(tarfile)                     # strict header validation
    files = Dict(h.path => h for h in headers if h.type == :file)
    Set(keys(files)) == Set(keys(tree)) || error("$tarfile: wrong file set")
    for (p, c) in tree
        files[p].size == length(c) || error("$tarfile: size of $p")
        files[p].mode == 0o644 || error("$tarfile: mode of $p")
    end
    mktempdir() do dir
        out = joinpath(dir, "x")
        Tar.extract(tarfile, out)
        for (p, c) in tree
            read(joinpath(out, p)) == c || error("$tarfile: content of $p")
        end
    end
    return headers
end

function main(args)
    outdir = args[1]
    mkpath(outdir)

    fs = fill_fs!(TarFS.InMemoryFileSystem(), TREE)
    open(io -> TarFS.write_tarball(fs, io), joinpath(outdir, "tarfs_jl.tar"), "w")
    TarFS.write_tarball_gz(fs, joinpath(outdir, "tarfs_jl.tar.gz"))

    ext = fill_fs!(TarFS.InMemoryFileSystem(), TREE)
    TarFS.writefile(ext, LONG_PATH, "leafdata")
    TarFS.write_tarball_gz_viatar(ext, joinpath(outdir, "tarfs_jl_viatar.tar.gz"))

    mktempdir() do src
        for (p, c) in TREE
            f = joinpath(src, p)
            mkpath(dirname(f))
            write(f, c)
        end
        open(joinpath(outdir, "tar_create.tar.gz"), "w") do out
            gz = GzipCompressorStream(out)
            Tar.create(src, gz)
            write(gz, TranscodingStreams.TOKEN_END)
            flush(gz)
        end
    end

    if length(args) >= 3
        check_extracted(args[2], TREE)
        exttree = merge(TREE, Dict(LONG_PATH => Vector{UInt8}(codeunits("leafdata"))))
        headers = check_extracted(args[3], exttree)
        any(h -> h.type == :directory, headers) || error("no directory entries")
        read(args[2]) == read(joinpath(outdir, "tarfs_jl.tar")) ||
            error("default writer output differs from TarFS.write_tarball")
    end
    println("TARFS_JL_OK")
end

main(ARGS)
