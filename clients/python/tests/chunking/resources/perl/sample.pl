#!/usr/bin/perl
#
# Sample Perl file for testing code chunking.
#
# This file contains various Perl constructs to test AST parsing.

use strict;
use warnings;
use feature 'say';
use Carp qw(croak);

# Constants
use constant MAX_RETRIES => 3;
use constant DEFAULT_TIMEOUT => 30.0;

# Package for User
package User;

sub new {
    my ($class, %args) = @_;
    croak "ID is required" unless defined $args{id};

    my $self = {
        id    => $args{id},
        name  => $args{name} // '',
        email => $args{email},
    };

    return bless $self, $class;
}

sub id { shift->{id} }

sub name {
    my ($self, $value) = @_;
    if (defined $value) {
        $self->{name} = $value;
    }
    return $self->{name};
}

sub email {
    my ($self, $value) = @_;
    if (defined $value) {
        $self->{email} = $value;
    }
    return $self->{email};
}

sub get_display_name {
    my $self = shift;
    return $self->{name} || $self->{email} || $self->{id};
}

sub to_hash {
    my $self = shift;
    return {
        id    => $self->{id},
        name  => $self->{name},
        email => $self->{email},
    };
}

# Package for UserService
package UserService;

sub new {
    my ($class, %args) = @_;

    my $self = {
        users       => {},
        initialized => 0,
        config      => $args{config} // {},
    };

    return bless $self, $class;
}

sub initialize {
    my $self = shift;
    $self->{initialized} = 1;
    return 1;
}

sub is_ready {
    my $self = shift;
    return $self->{initialized};
}

sub create_user {
    my ($self, %args) = @_;

    croak "ID cannot be empty" unless defined $args{id} && $args{id} ne '';

    my $user = User->new(%args);
    $self->{users}{$args{id}} = $user;
    $self->_on_user_created($user);

    return $user;
}

sub get_user {
    my ($self, $id) = @_;
    return $self->{users}{$id};
}

sub delete_user {
    my ($self, $id) = @_;

    if (exists $self->{users}{$id}) {
        delete $self->{users}{$id};
        return 1;
    }
    return 0;
}

sub get_all_users {
    my $self = shift;
    return values %{$self->{users}};
}

sub _on_user_created {
    my ($self, $user) = @_;
    # Internal callback
}

# Back to main package
package main;

# Calculate factorial of n
sub calculate_factorial {
    my $n = shift;
    return 1 if $n <= 1;
    return $n * calculate_factorial($n - 1);
}

# Fetch data from URL (simulated)
sub fetch_data {
    my ($url, $timeout) = @_;
    $timeout //= DEFAULT_TIMEOUT;

    return {
        url     => $url,
        status  => 'ok',
        timeout => $timeout,
    };
}

# Process items with optional validation
sub process_items {
    my ($items, $validate) = @_;
    $validate //= 1;

    my @filtered = $validate
        ? grep { defined $_ && $_ ne '' } @$items
        : @$items;

    return [ map { lc(trim($_)) } @filtered ];
}

# Helper to trim whitespace
sub trim {
    my $str = shift;
    $str =~ s/^\s+|\s+$//g;
    return $str;
}

# Retry a block with exponential backoff
sub with_retry {
    my ($max_retries, $block) = @_;
    $max_retries //= MAX_RETRIES;

    my $retry = 0;
    while ($retry < $max_retries) {
        my $result = eval { $block->() };
        return $result unless $@;
        $retry++;
        sleep(2 ** $retry);
    }
    die "Max retries exceeded: $@";
}

# Main execution
sub main {
    my $service = UserService->new(config => { env => 'test' });
    $service->initialize();

    my $user = $service->create_user(
        id    => '1',
        name  => 'Test User',
        email => 'test@example.com'
    );
    say "Created user: " . $user->get_display_name();

    my $result = calculate_factorial(5);
    say "Factorial: $result";
}

# Run main if executed directly
main() if !caller();

1;  # Return true for module
