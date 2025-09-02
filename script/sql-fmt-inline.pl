#!/usr/bin/env perl

use strict;
use warnings;
use IPC::Open3;
use Symbol 'gensym';

foreach my $file (@ARGV) {
    # Read the entire file content
    local $/ = undef;  # Enable slurp mode
    open my $fh, '<', $file or die "Could not open '$file': $!";
    my $content = <$fh>;
    close $fh;

    # Perform the regex substitution

    # Normal strings
    $content =~ s/sqlx::query!\(\s*"([^"\\]*?(?:\\.[^"\\]*?)*?)"([^)]*?)\)/process_query("", $1, "", $2)/ge;
    # r#"..."# strings
    $content =~ s/sqlx::query!\(\s*r#"(.*?)"#([^)]*?)\)/process_query("r#", $1, "#", $2)/ge;

    # Write the modified content back to the file
    open my $out_fh, '>', $file or die "Could not open '$file': $!";
    print $out_fh $content;
    close $out_fh;
}

sub process_query {
    my ($s_prefix, $query, $s_suffix, $args) = @_;

    # Create a pipe to the sleek command
    my $stderr = gensym;  # Create a symbol for STDERR
    my $pid = open3(my $stdin, my $stdout, $stderr, 'sleek');

    # Remove escape sequences
    $query =~ s/\\\\/\\/g;
    $query =~ s/\\'/'/g;
    $query =~ s/\\"/"/g;
    # Send the query to sleek
    print $stdin "$query\n";
    close $stdin;  # Close the input to signal completion

    # Read the output from sleek
    my $result = do { local $/; <$stdout> };
    close $stdout;

    # Check for errors
    my $error = do { local $/; <$stderr> };
    close $stderr;
    if ($error) {
        warn "Error from sleek: $error";
    }

    # Re-escape characters
    if ($s_prefix ne "r#") {
        $result =~ s/\\/\\\\/g;
        $result =~ s/'/\\'/g;
        $result =~ s/"/\\"/g;
    }

    # Return the modified string
    return "sqlx::query!($s_prefix\"\n$result\"$s_suffix$args)";
}
