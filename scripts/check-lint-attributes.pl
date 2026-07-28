#!/usr/bin/env perl

use strict;
use warnings;

my @files = `rg --files -g '*.rs' crates templates`;
my @violations;

for my $file (@files) {
    chomp $file;
    open my $source, '<', $file or die "failed to read $file: $!\n";
    local $/;
    my $contents = <$source>;

    while ($contents =~ /#!?\[\s*(allow|expect)\s*\((.*?)\)\s*\]/sg) {
        my ($attribute, $arguments) = ($1, $2);
        my $prefix = substr($contents, 0, $-[0]);
        my $line = 1 + ($prefix =~ tr/\n//);

        if ($attribute eq 'allow') {
            push @violations, "$file:$line: use #[expect(..., reason = \"...\")] instead of #[allow(...)]";
        }
        elsif ($arguments !~ /\breason\s*=/) {
            push @violations, "$file:$line: #[expect(...)] must include reason = \"...\"";
        }
    }
}

if (@violations) {
    print "Lint attribute policy violations:\n", join("\n", @violations), "\n";
    exit 1;
}
